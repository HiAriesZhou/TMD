mod ansi;
mod claude;
mod cli;
mod clipboard;
mod config;
mod large_headings;
mod reader;
mod reader_config;
mod reader_copy;
mod reader_native;
mod reader_outline;
mod reader_search;
#[cfg(test)]
mod snapshots;
mod source;
mod stream;
mod terminal_io;
mod theme_select;

use std::fs::File;
use std::io::{self, IsTerminal, Read, Write};
use std::path::Path;
use std::process::ExitCode;

use clap::Parser;
use tmd_core::{RenderOptions, Streamer, Theme, render_with_options};

use crate::cli::{Cli, InputFormat, resolve_width};
use crate::source::Source;

fn main() -> ExitCode {
    let cli = Cli::parse();
    let width = resolve_width(cli.width, terminal_width());
    // Picked before any input is read: `auto` may ask the terminal.
    let theme = match pick_theme(&cli.theme) {
        Ok(theme) => theme,
        Err(err) => {
            eprintln!("tmd: {err}");
            return ExitCode::FAILURE;
        }
    };

    if should_stream(&cli) {
        return match stream::run(
            Box::new(io::stdin()),
            move |width| make_source(cli.from, width, theme, cli.debug),
            cli.width,
        ) {
            Ok(code) => ExitCode::from(code),
            Err(err) => {
                eprintln!("tmd: {err}");
                ExitCode::FAILURE
            }
        };
    }

    let input: Box<dyn Read> = match &cli.file {
        Some(path) => match open_file(path) {
            Ok(file) => Box::new(file),
            Err(err) => {
                eprintln!("tmd: {}: {err}", path.display());
                return ExitCode::FAILURE;
            }
        },
        None => Box::new(io::stdin().lock()),
    };
    let md = match read_text(input) {
        Ok(md) => md,
        Err(err) => {
            eprintln!("tmd: {err}");
            return ExitCode::FAILURE;
        }
    };

    if reader::eligible(&cli) {
        let mode = match reader_config::load(cli.reader, config::config_dir().as_deref()) {
            Ok(mode) => mode,
            Err(err) => {
                eprintln!("tmd: {err}");
                return ExitCode::FAILURE;
            }
        };
        return match reader::run(&cli, &md, &theme, mode) {
            Ok(code) => ExitCode::from(code),
            Err(err) => {
                eprintln!("tmd: {err}");
                ExitCode::FAILURE
            }
        };
    }

    let large = large_headings::enabled(
        &cli,
        io::stdout().is_terminal(),
        crossterm::terminal::size().ok(),
        width,
    );
    let lines = match cli.from {
        InputFormat::Text => render_with_options(
            &md,
            width,
            &theme,
            RenderOptions {
                large_headings: large,
            },
        ),
        InputFormat::ClaudeJson => {
            let mut source = make_source(cli.from, width, theme, cli.debug);
            let mut lines = source.push(&md).committed;
            lines.extend(source.finish().committed);
            for diagnostic in source.take_diagnostics() {
                eprintln!("{diagnostic}");
            }
            lines
        }
    };
    let hyperlinks = io::stdout().is_terminal();
    let result = if large {
        large_headings::write(&lines)
    } else {
        write_out(io::stdout().lock(), &ansi::to_ansi(&lines, hyperlinks))
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) if large && err.kind() == io::ErrorKind::Interrupted => {
            ExitCode::from(large_headings::interrupted_exit_code())
        }
        // The reader went away (e.g. `tmd file | head`); nothing left to do.
        Err(err) if err.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("tmd: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Streams when markdown is piped in and the output is a terminal: the
/// input may be an AI tool still writing. Otherwise the input is read to
/// the end and printed once (`--ansi` asks for that explicitly).
fn should_stream(cli: &Cli) -> bool {
    cli.file.is_none() && !cli.ansi && !io::stdin().is_terminal() && io::stdout().is_terminal()
}

fn make_source(format: InputFormat, width: u16, theme: Theme, debug: bool) -> Box<dyn Source> {
    match format {
        InputFormat::Text => Box::new(Streamer::new(width, theme)),
        InputFormat::ClaudeJson => Box::new(claude::ClaudeStream::new(width, theme, debug)),
    }
}

/// The theme `--theme` names; `auto` picks default-dark or default-light
/// by the terminal's background.
fn pick_theme(name: &str) -> Result<Theme, String> {
    let name = if name == theme_select::AUTO {
        let dark = theme_select::auto_is_dark(
            io::stdout().is_terminal(),
            query_dark_background,
            std::env::var("COLORFGBG").ok().as_deref(),
        );
        if dark {
            "default-dark"
        } else {
            "default-light"
        }
    } else {
        name
    };
    theme_select::load(name, config::config_dir().as_deref())
}

/// Asks the terminal whether its background is dark, waiting at most
/// 300 ms.
fn query_dark_background() -> Option<bool> {
    let mut options = terminal_colorsaurus::QueryOptions::default();
    options.timeout = std::time::Duration::from_millis(300);
    terminal_colorsaurus::theme_mode(options)
        .ok()
        .map(|mode| mode == terminal_colorsaurus::ThemeMode::Dark)
}

/// Opens `path` for reading. Directories are rejected here because on Unix
/// opening one succeeds and only the later read fails, without the path.
fn open_file(path: &Path) -> io::Result<File> {
    let file = File::open(path)?;
    if file.metadata()?.is_dir() {
        return Err(io::ErrorKind::IsADirectory.into());
    }
    Ok(file)
}

/// Reads all input; invalid UTF-8 becomes U+FFFD instead of failing.
fn read_text(mut input: impl Read) -> io::Result<String> {
    let mut bytes = Vec::new();
    input.read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Columns of the terminal stdout is attached to, or `None` when stdout is
/// not a terminal.
fn terminal_width() -> Option<u16> {
    if !io::stdout().is_terminal() {
        return None;
    }
    crossterm::terminal::size()
        .ok()
        .map(|(cols, _)| cols)
        .filter(|&cols| cols > 0)
}

fn write_out(mut output: impl Write, text: &str) -> io::Result<()> {
    output.write_all(text.as_bytes())?;
    output.flush()
}
