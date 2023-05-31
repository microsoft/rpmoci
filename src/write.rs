//! Functions for writing messages.
//!
//! Copyright (C) Microsoft Corporation.
//!
//! This program is free software: you can redistribute it and/or modify
//! it under the terms of the GNU General Public License as published by
//! the Free Software Foundation, either version 3 of the License, or
//! (at your option) any later version.
//!
//! This program is distributed in the hope that it will be useful,
//! but WITHOUT ANY WARRANTY; without even the implied warranty of
//! MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
//! GNU General Public License for more details.
//!
//! You should have received a copy of the GNU General Public License
//! along with this program.  If not, see <https://www.gnu.org/licenses/>.
use std::fmt::Display;
use std::io::{self, Write};
use termcolor::{Color, ColorChoice, ColorSpec, StandardStream, WriteColor};

fn msg(label: &str, message: impl Display, color: &ColorSpec) -> io::Result<()> {
    let mut stderr = StandardStream::stderr(ColorChoice::Auto);
    stderr.set_color(color)?;
    write!(&mut stderr, "{:>20} ", label)?;
    stderr.set_color(ColorSpec::new().set_fg(None))?;
    writeln!(&mut stderr, "{}", message)?;
    Ok(())
}

/// Write an ok message to stderr
///
/// # Errors
///
/// Will return `Err` if a problem is encountered writing to stderr
pub fn ok(label: &str, message: impl Display) -> io::Result<()> {
    msg(label, message, ColorSpec::new().set_fg(Some(Color::Green)))
}

/// Write an error message to stderr
///
/// # Errors
///
/// Will return `Err` if a problem is encountered writing to stderr
pub fn error(label: &str, message: impl Display) -> io::Result<()> {
    msg(
        label,
        message,
        ColorSpec::new().set_fg(Some(Color::Red)).set_bold(true),
    )
}
