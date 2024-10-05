use std::io::Write;

use super::CompressionAlgorithm;
use anyhow::Result;
use ocidir::{GzipLayerWriter, Layer, OciDir, ZstdLayerWriter};

pub(super) enum LayerWriter<'a> {
    Gzip(GzipLayerWriter<'a>),
    Zstd(ZstdLayerWriter<'a>),
}

impl<'a> LayerWriter<'a> {
    pub fn new(
        ocidir: &'a OciDir,
        compression_algorithm: CompressionAlgorithm,
        compression_level: Option<i32>,
    ) -> Result<Self> {
        Ok(match compression_algorithm {
            CompressionAlgorithm::Gzip => Self::Gzip(ocidir.create_gzip_layer(
                compression_level.map(|l| flate2::Compression::new(l.try_into().unwrap())),
            )?),
            CompressionAlgorithm::Zstd => Self::Zstd(ocidir.create_layer_zstd_multithread(
                compression_level,
                num_cpus::get().try_into().unwrap(),
            )?),
        })
    }

    pub fn complete(self) -> Result<Layer> {
        match self {
            Self::Gzip(writer) => Ok(writer.complete()?),
            Self::Zstd(writer) => Ok(writer.complete()?),
        }
    }
}

impl<'a> Write for LayerWriter<'a> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Gzip(writer) => writer.write(buf),
            Self::Zstd(writer) => writer.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Gzip(writer) => writer.flush(),
            Self::Zstd(writer) => writer.flush(),
        }
    }
}
