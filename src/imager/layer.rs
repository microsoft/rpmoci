use std::{io::Write, str::FromStr as _};

use anyhow::Result;
use ocidir::{
    oci_spec::image::{MediaType, Sha256Digest},
    BlobWriter, GzipLayerWriter, Layer, OciDir,
};
use zstd::Encoder;

use super::{sha256_writer::Sha256Writer, CompressionAlgorithm};

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
            CompressionAlgorithm::Zstd => {
                Self::Zstd(ZstdLayerWriter::new(ocidir, compression_level)?)
            }
        })
    }

    pub fn complete(self) -> Result<Layer> {
        match self {
            Self::Gzip(writer) => Ok(writer.complete()?),
            Self::Zstd(writer) => writer.complete(),
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

/// A writer for a zstd compressed layer.
pub struct ZstdLayerWriter<'a>(Sha256Writer<Encoder<'static, BlobWriter<'a>>>);

impl<'a> ZstdLayerWriter<'a> {
    /// Create new zstd layer.
    fn new(ocidir: &'a OciDir, compression: Option<i32>) -> Result<Self> {
        let bw = ocidir.create_blob()?;
        let mut encoder = Encoder::new(bw, compression.unwrap_or(0))?;
        // Set the number of workers to the number of CPUs
        // when in multi-threaded mode each zstd version is reproducible regardless of the number of threads
        let num_workers = num_cpus::get();
        encoder.set_parameter(zstd::zstd_safe::CParameter::NbWorkers(
            num_workers.try_into()?,
        ))?;
        Ok(Self(Sha256Writer::new(encoder)))
    }

    /// Finish writing the layer.
    pub fn complete(self) -> Result<Layer> {
        let (digest, encoder) = self.0.finish();
        let uncompressed_sha256 = Sha256Digest::from_str(&digest)?;
        let blob = encoder.finish()?.complete()?;
        Ok(Layer {
            uncompressed_sha256,
            blob,
            media_type: MediaType::ImageLayerZstd,
        })
    }
}

impl<'a> Write for ZstdLayerWriter<'a> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}
