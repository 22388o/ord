#![allow(clippy::type_complexity)]

use {
  self::{command_builder::CommandBuilder, expected::Expected, test_server::TestServer},
  bitcoin::{blockdata::constants::COIN_VALUE, Address, Network, OutPoint, Txid},
  executable_path::executable_path,
  pretty_assertions::assert_eq as pretty_assert_eq,
  regex::Regex,
  reqwest::{StatusCode, Url},
  std::{
    fs,
    net::TcpListener,
    path::Path,
    process::{Child, Command, Stdio},
    str, thread,
    time::Duration,
  },
  tempfile::TempDir,
  unindent::Unindent,
};

macro_rules! assert_regex_match {
  ($string:expr, $pattern:expr $(,)?) => {
    let regex = Regex::new(&format!("^(?s){}$", $pattern)).unwrap();
    let string = $string;

    if !regex.is_match(string.as_ref()) {
      panic!(
        "Regex:\n\n{}\n\n…did not match string:\n\n{}",
        regex, string
      );
    }
  };
}

fn reveal_txid_from_inscribe_stdout(stdout: &str) -> Txid {
  stdout
    .lines()
    .nth(1)
    .unwrap()
    .split('\t')
    .nth(1)
    .unwrap()
    .parse()
    .unwrap()
}

mod command_builder;
mod epochs;
mod expected;
mod find;
mod index;
mod info;
mod list;
mod parse;
mod server;
mod subsidy;
mod supply;
mod test_server;
mod traits;
mod version;
mod wallet;
