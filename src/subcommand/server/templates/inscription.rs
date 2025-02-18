use super::*;

#[derive(Boilerplate)]
pub(crate) struct InscriptionHtml {
  pub(crate) inscription_id: InscriptionId,
  pub(crate) inscription: Inscription,
  pub(crate) satpoint: SatPoint,
}

impl PageContent for InscriptionHtml {
  fn title(&self) -> String {
    format!("Inscription {}", self.inscription_id)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn txt_inscription() {
    pretty_assert_eq!(
      InscriptionHtml {
        inscription_id: InscriptionId::from_str(
          "ec90757eb3b164aa43fc548faa2fa0c52025494f2c15d5ddf11260b4034ac6dc"
        )
        .unwrap(),
        inscription: inscription("text/plain;charset=utf-8", "HELLOWORLD"),
        satpoint: satpoint(1, 0),
      }
      .to_string(),
      "
        <h1>Inscription ec90757eb3b164aa43fc548faa2fa0c52025494f2c15d5ddf11260b4034ac6dc</h1>
        <dl>
          <dt>content size</dt>
          <dd>10 bytes</dd>
          <dt>content type</dt>
          <dd>text/plain;charset=utf-8</dd>
          <dt>location</dt>
          <dd>1111111111111111111111111111111111111111111111111111111111111111:1:0</dd>
        </dl>
        HELLOWORLD
      "
      .unindent()
    );
  }

  #[test]
  fn png_inscription() {
    pretty_assert_eq!(
      InscriptionHtml {
        inscription_id: InscriptionId::from_str("ec90757eb3b164aa43fc548faa2fa0c52025494f2c15d5ddf11260b4034ac6dc").unwrap(),
        inscription: inscription("image/png", [1; 100]),
        satpoint: satpoint(1, 0),
      }
      .to_string(),
      "
        <h1>Inscription ec90757eb3b164aa43fc548faa2fa0c52025494f2c15d5ddf11260b4034ac6dc</h1>
        <dl>
          <dt>content size</dt>
          <dd>100 bytes</dd>
          <dt>content type</dt>
          <dd>image/png</dd>
          <dt>location</dt>
          <dd>1111111111111111111111111111111111111111111111111111111111111111:1:0</dd>
        </dl>
        <img src='data:image/png;base64,AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQ=='>
      "
      .unindent()
    );
  }

  #[test]
  fn empty_inscription() {
    pretty_assert_eq!(
      InscriptionHtml {
        inscription_id: InscriptionId::from_str(
          "ec90757eb3b164aa43fc548faa2fa0c52025494f2c15d5ddf11260b4034ac6dc"
        )
        .unwrap(),
        inscription: Inscription::new(None, None),
        satpoint: satpoint(1, 0),
      }
      .to_string(),
      "
        <h1>Inscription ec90757eb3b164aa43fc548faa2fa0c52025494f2c15d5ddf11260b4034ac6dc</h1>
        <dl>
          <dt>location</dt>
          <dd>1111111111111111111111111111111111111111111111111111111111111111:1:0</dd>
        </dl>
        UNKNOWN
      "
      .unindent()
    );
  }
}
