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
use openssl::sha::Sha256;
use std::io::{Result, Write};

/// Wraps a writer and calculates the sha256 digest of data written to the inner writer
pub(crate) struct Sha256Writer<W> {
    writer: W,
    sha: Sha256,
}

impl<W> Sha256Writer<W> {
    pub(crate) fn new(writer: W) -> Self {
        Self {
            writer,
            sha: Sha256::new(),
        }
    }

    /// Return the hex encoded sha256 digest of the written data, and the underlying writer
    pub(crate) fn finish(self) -> (String, W) {
        let digest = hex::encode(self.sha.finish());
        (digest, self.writer)
    }
}

impl<W> Write for Sha256Writer<W>
where
    W: Write,
{
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        let len = self.writer.write(buf)?;
        self.sha.update(&buf[..len]);
        Ok(len)
    }

    fn flush(&mut self) -> Result<()> {
        self.writer.flush()
    }
}
