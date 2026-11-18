//! Content decoding, for display only: forwarded bytes are never changed.

use std::io::{self, Read};

/// Upper bound on decoded output, against decompression bombs.
const MAX_DECODED: u64 = 32 * 1024 * 1024;

/// Undoes `Content-Encoding` (e.g. `gzip`, `br, gzip`). `None` if there is
/// nothing to undo or the encoding is unknown. A truncated capture decodes as
/// far as it goes.
pub(crate) fn decode(body: &[u8], content_encoding: &str) -> Option<Vec<u8>> {
    let encodings: Vec<String> = content_encoding
        .split(',')
        .map(|e| e.trim().to_ascii_lowercase())
        .filter(|e| !e.is_empty() && e != "identity")
        .collect();
    if encodings.is_empty() || body.is_empty() {
        return None;
    }
    let mut data = body.to_vec();
    for encoding in encodings.iter().rev() {
        data = decode_one(&data, encoding).ok()?; // applied in order, so undo in reverse
    }
    Some(data)
}

fn decode_one(data: &[u8], encoding: &str) -> io::Result<Vec<u8>> {
    let reader: Box<dyn Read + '_> = match encoding {
        "gzip" | "x-gzip" => Box::new(flate2::read::MultiGzDecoder::new(data)),
        "deflate" if looks_like_zlib(data) => Box::new(flate2::read::ZlibDecoder::new(data)),
        "deflate" => Box::new(flate2::read::DeflateDecoder::new(data)), // raw deflate, seen in the wild
        "br" => Box::new(brotli_decompressor::Decompressor::new(data, 4096)),
        "zstd" => {
            Box::new(ruzstd::decoding::StreamingDecoder::new(data).map_err(io::Error::other)?)
        }
        _ => return Err(io::ErrorKind::Unsupported.into()),
    };
    let mut out = Vec::new();
    match reader.take(MAX_DECODED).read_to_end(&mut out) {
        Ok(_) => Ok(out),
        Err(_) if !out.is_empty() => Ok(out), // truncated capture: keep what decoded
        Err(e) => Err(e),
    }
}

fn looks_like_zlib(data: &[u8]) -> bool {
    data.len() >= 2
        && data[0] & 0x0f == 8
        && (u16::from(data[0]) << 8 | u16::from(data[1])) % 31 == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn gzip(data: &[u8]) -> Vec<u8> {
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(data).unwrap();
        enc.finish().unwrap()
    }

    #[test]
    fn gzip_and_stacked_encodings() {
        assert_eq!(decode(&gzip(b"hello"), "gzip").unwrap(), b"hello");
        assert_eq!(
            decode(&gzip(&gzip(b"twice")), "gzip, gzip").unwrap(),
            b"twice"
        );
    }

    #[test]
    fn deflate_in_both_flavours() {
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(b"zlib").unwrap();
        assert_eq!(decode(&z.finish().unwrap(), "deflate").unwrap(), b"zlib");
        let mut raw =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        raw.write_all(b"raw").unwrap();
        assert_eq!(decode(&raw.finish().unwrap(), "deflate").unwrap(), b"raw");
    }

    #[test]
    fn truncated_capture_decodes_partially() {
        let text = "abcdefghij".repeat(2000);
        let packed = gzip(text.as_bytes());
        let partial = decode(&packed[..packed.len() / 2], "gzip").unwrap();
        assert!(!partial.is_empty() && text.as_bytes().starts_with(&partial));
    }

    #[test]
    fn nothing_to_do() {
        assert_eq!(decode(b"x", "identity"), None);
        assert_eq!(decode(b"x", ""), None);
        assert_eq!(decode(b"x", "compress"), None);
    }
}
