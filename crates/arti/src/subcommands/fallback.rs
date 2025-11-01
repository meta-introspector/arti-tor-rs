//! A placeholder fallback subcommand.

use crate::subcommands::prompt;
use crate::{Result, TorClient};

use anyhow::{Context, anyhow};
use clap::{ArgMatches, Args, FromArgMatches, Parser, Subcommand, ValueEnum};
use safelog::DisplayRedacted;
use tor_rtcompat::Runtime;

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::str::FromStr;

/// Fallback subcommand placeholder.
#[derive(Parser, Debug)]
pub(crate) enum FallbackSubcommands {
    /// Fallback subcommand placeholder for an Arti client.
    #[command(subcommand)]
    Fallback,
}

/// Run the `hsc` subcommand.
pub(crate) fn run(feature: &str) -> Result<()> {
    Err(anyhow::anyhow!(format!("subcommand '{}' not supported in this build (hint: recompile arti with the `{}` feature)", feature, feature)))
}
