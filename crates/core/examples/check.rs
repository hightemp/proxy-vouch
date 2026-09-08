use proxy_vouch_core::{
    checker::{check, Control},
    model::CheckSettings,
    parser::parse_line,
};
use serde::Deserialize;
use std::{
    io::{self, Read},
    path::PathBuf,
    sync::Arc,
};

#[derive(Deserialize)]
struct Input {
    proxies: Vec<String>,
    settings: CheckSettings,
    ca_file: Option<PathBuf>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    let input: Input = serde_json::from_str(&input)?;
    input.settings.validate()?;
    let mut control = Control::default();
    control.ca_file = input.ca_file;
    let control = Arc::new(control);
    let mut results = Vec::new();
    for line in input.proxies {
        results.push(check(
            &parse_line(&line, false)?,
            &input.settings,
            &control,
            None,
        ));
    }
    println!("{}", serde_json::to_string(&results)?);
    Ok(())
}
