use {super::*, std::sync::mpsc};

pub(crate) struct BlockData {
  header: BlockHeader,
  txdata: Vec<(Transaction, Txid)>,
}

impl From<Block> for BlockData {
  fn from(block: Block) -> Self {
    BlockData {
      header: block.header,
      txdata: block
        .txdata
        .into_iter()
        .map(|transaction| {
          let txid = transaction.txid();
          (transaction, txid)
        })
        .collect(),
    }
  }
}

pub struct Updater {
  cache: HashMap<OutPointArray, Vec<u8>>,
  height: u64,
  index_satoshis: bool,
  sat_ranges_since_flush: u64,
  outputs_cached: u64,
  outputs_inserted_since_flush: u64,
  outputs_traversed: u64,
}

impl Updater {
  pub(crate) fn update(index: &Index) -> Result {
    let wtx = index.begin_write()?;

    let height = wtx
      .open_table(HEIGHT_TO_BLOCK_HASH)?
      .range(0..)?
      .rev()
      .next()
      .map(|(height, _hash)| height + 1)
      .unwrap_or(0);

    wtx
      .open_table(WRITE_TRANSACTION_STARTING_BLOCK_COUNT_TO_TIMESTAMP)?
      .insert(
        &height,
        &SystemTime::now()
          .duration_since(SystemTime::UNIX_EPOCH)
          .map(|duration| duration.as_millis())
          .unwrap_or(0),
      )?;

    let mut updater = Self {
      cache: HashMap::new(),
      height,
      index_satoshis: index.has_satoshi_index()?,
      sat_ranges_since_flush: 0,
      outputs_cached: 0,
      outputs_inserted_since_flush: 0,
      outputs_traversed: 0,
    };

    updater.update_index(index, wtx)
  }

  pub(crate) fn update_index<'index>(
    &mut self,
    index: &'index Index,
    mut wtx: WriteTransaction<'index>,
  ) -> Result {
    let starting_height = index.client.get_block_count()? + 1;

    let mut progress_bar = if cfg!(test)
      || log_enabled!(log::Level::Info)
      || starting_height <= self.height
      || env::var_os("ORD_DISABLE_PROGRESS_BAR")
        .map(|value| value.len() > 0)
        .unwrap_or(false)
    {
      None
    } else {
      let progress_bar = ProgressBar::new(starting_height);
      progress_bar.set_position(self.height);
      progress_bar.set_style(
        ProgressStyle::with_template("[indexing blocks] {wide_bar} {pos}/{len}").unwrap(),
      );
      Some(progress_bar)
    };

    let rx = Self::fetch_blocks_from(index, self.height, self.index_satoshis)?;

    let mut uncommitted = 0;
    loop {
      let block = match rx.recv() {
        Ok(block) => block,
        Err(mpsc::RecvError) => break,
      };

      self.index_block(index, &mut wtx, block)?;

      if let Some(progress_bar) = &mut progress_bar {
        progress_bar.inc(1);

        if progress_bar.position() > progress_bar.length().unwrap() {
          progress_bar.set_length(index.client.get_block_count()? + 1);
        }
      }

      uncommitted += 1;

      if uncommitted == 5000 {
        self.commit(wtx)?;
        uncommitted = 0;
        wtx = index.begin_write()?;
        let height = wtx
          .open_table(HEIGHT_TO_BLOCK_HASH)?
          .range(0..)?
          .rev()
          .next()
          .map(|(height, _hash)| height + 1)
          .unwrap_or(0);
        if height != self.height {
          // another update has run between committing and beginning the new
          // write transaction
          break;
        }
        wtx
          .open_table(WRITE_TRANSACTION_STARTING_BLOCK_COUNT_TO_TIMESTAMP)?
          .insert(
            &self.height,
            &SystemTime::now()
              .duration_since(SystemTime::UNIX_EPOCH)
              .map(|duration| duration.as_millis())
              .unwrap_or(0),
          )?;
      }

      if INTERRUPTS.load(atomic::Ordering::Relaxed) > 0 {
        break;
      }
    }

    if uncommitted > 0 {
      self.commit(wtx)?;
    }

    if let Some(progress_bar) = &mut progress_bar {
      progress_bar.finish_and_clear();
    }

    Ok(())
  }

  fn fetch_blocks_from(
    index: &Index,
    mut height: u64,
    index_satoshis: bool,
  ) -> Result<mpsc::Receiver<BlockData>> {
    let (tx, rx) = mpsc::sync_channel(32);

    let height_limit = index.height_limit;

    let client =
      Client::new(&index.rpc_url, index.auth.clone()).context("failed to connect to RPC URL")?;

    let with_transactions = index_satoshis || index.chain != Chain::Mainnet;

    thread::spawn(move || loop {
      if let Some(height_limit) = height_limit {
        if height >= height_limit {
          break;
        }
      }

      match Self::get_block_with_retries(&client, height, with_transactions) {
        Ok(Some(block)) => {
          if let Err(err) = tx.send(block.into()) {
            log::info!("Block receiver disconnected: {err}");
            break;
          }
          height += 1;
        }
        Ok(None) => break,
        Err(err) => {
          log::error!("Failed to fetch block {height}: {err}");
          break;
        }
      }
    });

    Ok(rx)
  }

  pub(crate) fn get_block_with_retries(
    client: &Client,
    height: u64,
    with_transactions: bool,
  ) -> Result<Option<Block>> {
    let mut errors = 0;
    loop {
      match client
        .get_block_hash(height)
        .into_option()
        .and_then(|option| {
          option
            .map(|hash| {
              if with_transactions {
                Ok(client.get_block(&hash)?)
              } else {
                Ok(Block {
                  header: client.get_block_header(&hash)?,
                  txdata: Vec::new(),
                })
              }
            })
            .transpose()
        }) {
        Err(err) => {
          if cfg!(test) {
            return Err(err);
          }

          errors += 1;
          let seconds = 1 << errors;
          log::error!("failed to fetch block {height}, retrying in {seconds}s: {err}");

          if seconds > 120 {
            log::error!("would sleep for more than 120s, giving up");
            return Err(err);
          }

          thread::sleep(Duration::from_secs(seconds));
        }
        Ok(result) => return Ok(result),
      }
    }
  }

  pub(crate) fn index_block(
    &mut self,
    index: &Index,
    wtx: &mut WriteTransaction,
    block: BlockData,
  ) -> Result<()> {
    let mut height_to_block_hash = wtx.open_table(HEIGHT_TO_BLOCK_HASH)?;

    let start = Instant::now();
    let mut sat_ranges_written = 0;
    let mut outputs_in_block = 0;

    let time = Utc.timestamp_opt(block.header.time.into(), 0).unwrap();

    log::info!(
      "Block {} at {} with {} transactions…",
      self.height,
      time,
      block.txdata.len()
    );

    if let Some(prev_height) = self.height.checked_sub(1) {
      let prev_hash = height_to_block_hash.get(&prev_height)?.unwrap();

      if prev_hash != block.header.prev_blockhash.as_ref() {
        index.reorged.store(true, Ordering::Relaxed);
        return Err(anyhow!("reorg detected at or before {prev_height}"));
      }
    }

    let mut inscription_id_to_satpoint = wtx.open_table(INSCRIPTION_ID_TO_SATPOINT)?;
    let mut satpoint_to_inscription_id = wtx.open_table(SATPOINT_TO_INSCRIPTION_ID)?;

    if self.index_satoshis {
      let mut sat_to_inscription_id = wtx.open_table(SAT_TO_INSCRIPTION_ID)?;
      let mut sat_to_satpoint = wtx.open_table(SAT_TO_SATPOINT)?;
      let mut outpoint_to_sat_ranges = wtx.open_table(OUTPOINT_TO_SAT_RANGES)?;

      let mut coinbase_inputs = VecDeque::new();

      let h = Height(self.height);
      if h.subsidy() > 0 {
        let start = h.starting_sat();
        coinbase_inputs.push_front((start.n(), (start + h.subsidy()).n()));
        self.sat_ranges_since_flush += 1;
      }

      for (tx_offset, (tx, txid)) in block.txdata.iter().enumerate().skip(1) {
        log::trace!("Indexing transaction {tx_offset}…");

        let mut input_sat_ranges = VecDeque::new();

        for input in &tx.input {
          let key = encode_outpoint(input.previous_output);

          let sat_ranges = match self.cache.remove(&key) {
            Some(sat_ranges) => {
              self.outputs_cached += 1;
              sat_ranges
            }
            None => outpoint_to_sat_ranges
              .remove(&key)?
              .ok_or_else(|| anyhow!("Could not find outpoint {} in index", input.previous_output))?
              .to_value()
              .to_vec(),
          };

          for chunk in sat_ranges.chunks_exact(11) {
            input_sat_ranges.push_back(Index::decode_sat_range(chunk.try_into().unwrap()));
          }
        }

        self.index_transaction_sats(
          tx,
          *txid,
          &mut sat_to_satpoint,
          &mut sat_to_inscription_id,
          &mut inscription_id_to_satpoint,
          &mut satpoint_to_inscription_id,
          &mut input_sat_ranges,
          &mut sat_ranges_written,
          &mut outputs_in_block,
        )?;

        coinbase_inputs.extend(input_sat_ranges);
      }

      if let Some((tx, txid)) = block.txdata.get(0) {
        self.index_transaction_sats(
          tx,
          *txid,
          &mut sat_to_satpoint,
          &mut sat_to_inscription_id,
          &mut inscription_id_to_satpoint,
          &mut satpoint_to_inscription_id,
          &mut coinbase_inputs,
          &mut sat_ranges_written,
          &mut outputs_in_block,
        )?;
      }
    } else {
      for (tx, txid) in &block.txdata {
        self.index_transaction_inscriptions(
          tx,
          *txid,
          &mut inscription_id_to_satpoint,
          &mut satpoint_to_inscription_id,
        )?;
      }
    }

    height_to_block_hash.insert(
      &self.height,
      &block.header.block_hash().as_hash().into_inner(),
    )?;

    self.height += 1;
    self.outputs_traversed += outputs_in_block;

    log::info!(
      "Wrote {sat_ranges_written} sat ranges from {outputs_in_block} outputs in {} ms",
      (Instant::now() - start).as_millis(),
    );

    Ok(())
  }

  pub(crate) fn index_transaction_inscriptions(
    &mut self,
    tx: &Transaction,
    txid: Txid,
    inscription_id_to_satpoint: &mut Table<&InscriptionIdArray, &SatPointArray>,
    satpoint_to_inscription_id: &mut Table<&SatPointArray, &InscriptionIdArray>,
  ) -> Result<bool> {
    let inscribed = Inscription::from_transaction(tx).is_some();

    if inscribed {
      let satpoint = encode_satpoint(SatPoint {
        outpoint: OutPoint { txid, vout: 0 },
        offset: 0,
      });

      inscription_id_to_satpoint.insert(txid.as_inner(), &satpoint)?;
      satpoint_to_inscription_id.insert(&satpoint, txid.as_inner())?;
    };

    for tx_in in &tx.input {
      let outpoint = tx_in.previous_output;
      let start = encode_satpoint(SatPoint {
        outpoint,
        offset: 0,
      });

      let end = encode_satpoint(SatPoint {
        outpoint,
        offset: u64::MAX,
      });

      let inscription_ids: Vec<(SatPointArray, InscriptionIdArray)> = satpoint_to_inscription_id
        .range(start..=end)?
        .map(|(satpoint, id)| (*satpoint, *id))
        .collect();

      for (old_satpoint, inscription_id) in inscription_ids {
        let new_satpoint = encode_satpoint(SatPoint {
          outpoint: OutPoint { txid, vout: 0 },
          offset: 0,
        });

        satpoint_to_inscription_id.remove(&old_satpoint)?;
        satpoint_to_inscription_id.insert(&new_satpoint, &inscription_id)?;
        inscription_id_to_satpoint.insert(&inscription_id, &new_satpoint)?;
      }
    }

    Ok(inscribed)
  }

  pub(crate) fn index_transaction_sats(
    &mut self,
    tx: &Transaction,
    txid: Txid,
    sat_to_satpoint: &mut Table<u64, &SatPointArray>,
    sat_to_inscription_id: &mut Table<u64, &InscriptionIdArray>,
    inscription_id_to_satpoint: &mut Table<&InscriptionIdArray, &SatPointArray>,
    satpoint_to_inscription_id: &mut Table<&SatPointArray, &InscriptionIdArray>,
    input_sat_ranges: &mut VecDeque<(u64, u64)>,
    sat_ranges_written: &mut u64,
    outputs_traversed: &mut u64,
  ) -> Result {
    if self.index_transaction_inscriptions(
      tx,
      txid,
      inscription_id_to_satpoint,
      satpoint_to_inscription_id,
    )? {
      if let Some((start, _end)) = input_sat_ranges.get(0) {
        sat_to_inscription_id.insert(&start, txid.as_inner())?;
      }
    }

    for (vout, output) in tx.output.iter().enumerate() {
      let outpoint = OutPoint {
        vout: vout.try_into().unwrap(),
        txid,
      };
      let mut sats = Vec::new();

      let mut remaining = output.value;
      while remaining > 0 {
        let range = input_sat_ranges
          .pop_front()
          .ok_or_else(|| anyhow!("insufficient inputs for transaction outputs"))?;

        if !Sat(range.0).is_common() {
          sat_to_satpoint.insert(
            &range.0,
            &encode_satpoint(SatPoint {
              outpoint,
              offset: output.value - remaining,
            }),
          )?;
        }

        let count = range.1 - range.0;

        let assigned = if count > remaining {
          self.sat_ranges_since_flush += 1;
          let middle = range.0 + remaining;
          input_sat_ranges.push_front((middle, range.1));
          (range.0, middle)
        } else {
          range
        };

        let base = assigned.0;
        let delta = assigned.1 - assigned.0;

        let n = u128::from(base) | u128::from(delta) << 51;

        sats.extend_from_slice(&n.to_le_bytes()[0..11]);

        remaining -= assigned.1 - assigned.0;

        *sat_ranges_written += 1;
      }

      *outputs_traversed += 1;

      self.cache.insert(encode_outpoint(outpoint), sats);
      self.outputs_inserted_since_flush += 1;
    }

    Ok(())
  }

  pub(crate) fn commit(&mut self, wtx: WriteTransaction) -> Result {
    log::info!(
      "Committing at block height {}, {} outputs traversed, {} in map, {} cached",
      self.height,
      self.outputs_traversed,
      self.cache.len(),
      self.outputs_cached
    );

    if self.index_satoshis {
      log::info!(
        "Flushing {} entries ({:.1}% resulting from {} insertions) from memory to database",
        self.cache.len(),
        self.cache.len() as f64 / self.outputs_inserted_since_flush as f64 * 100.,
        self.outputs_inserted_since_flush,
      );

      let mut outpoint_to_sat_ranges = wtx.open_table(OUTPOINT_TO_SAT_RANGES)?;

      for (k, v) in &self.cache {
        outpoint_to_sat_ranges.insert(k, v)?;
      }

      self.cache.clear();
      self.outputs_inserted_since_flush = 0;
    }

    Index::increment_statistic(&wtx, Statistic::OutputsTraversed, self.outputs_traversed)?;
    self.outputs_traversed = 0;
    Index::increment_statistic(&wtx, Statistic::SatRanges, self.sat_ranges_since_flush)?;
    self.sat_ranges_since_flush = 0;
    Index::increment_statistic(&wtx, Statistic::Commits, 1)?;

    wtx.commit()?;
    Ok(())
  }
}
