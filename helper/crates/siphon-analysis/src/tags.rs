//! DJ-readable tempo/key tags that leave unrelated metadata alone, and the WAV
//! `acid` chunk DAWs use to tempo-sync loops.
//!
//! TKEY/INITIALKEY always hold the standard short name (Am, F#m, Bb), never
//! Camelot. MP4 `tmpo` is an integer; other formats keep 0.1 BPM precision.

use std::borrow::Cow;
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::{AudioFile, FileType};
use lofty::flac::FlacFile;
use lofty::id3::v2::{Frame, FrameId, Id3v2Tag, TextInformationFrame};
use lofty::iff::wav::WavFile;
use lofty::mp4::{Atom, AtomData, AtomIdent, DataType, Ilst, Mp4File};
use lofty::mpeg::MpegFile;
use lofty::ogg::tag::VorbisComments;
use lofty::ogg::{OggPictureStorage, OpusFile, VorbisFile};
use lofty::probe::Probe;
use lofty::tag::TagExt;
use lofty::TextEncoding;

use crate::notation::parse_key_names;

fn bpm_text(bpm: f64) -> String {
    let text = format!("{bpm:.1}");
    text.strip_suffix(".0").map(str::to_string).unwrap_or(text)
}

fn open(path: &Path) -> Result<std::fs::File, String> {
    std::fs::File::open(path).map_err(|e| e.to_string())
}

/// Update MP3/WAV ID3, MP4, FLAC, Ogg Opus or Ogg Vorbis tags in place.
pub fn write_tags(path: &Path, bpm: f64, key: &str) -> Result<(), String> {
    if !bpm.is_finite() || !(bpm > 0.0 && bpm <= 65535.0) {
        return Err("bpm must be finite, positive, and representable in MP4 tmpo".into());
    }
    let key = parse_key_names(key)?.key_short;
    let text = bpm_text(bpm);
    let kind = Probe::open(path)
        .map_err(|e| e.to_string())?
        .guess_file_type()
        .map_err(|e| e.to_string())?
        .file_type()
        .ok_or("Unsupported audio container for BPM/key tags")?;
    let options = ParseOptions::new().read_properties(false);
    let save = WriteOptions::default();
    let id3 = |mut tag: Id3v2Tag| -> Result<(), String> {
        for (id, value) in [("TBPM", text.clone()), ("TKEY", key.clone())] {
            tag.insert(Frame::Text(TextInformationFrame::new(FrameId::Valid(Cow::Borrowed(id)), TextEncoding::UTF8, value)));
        }
        tag.save_to_path(path, save).map_err(|e| e.to_string())
    };
    let vorbis = |mut tag: VorbisComments| -> Result<(), String> {
        tag.insert("BPM".into(), text.clone());
        tag.insert("INITIALKEY".into(), key.clone());
        tag.save_to_path(path, save).map_err(|e| e.to_string())
    };
    match kind {
        FileType::Mpeg => id3(MpegFile::read_from(&mut open(path)?, options).map_err(|e| e.to_string())?.id3v2().cloned().unwrap_or_default()),
        FileType::Wav => id3(WavFile::read_from(&mut open(path)?, options).map_err(|e| e.to_string())?.id3v2().cloned().unwrap_or_default()),
        FileType::Mp4 => {
            let mut tag: Ilst = Mp4File::read_from(&mut open(path)?, options).map_err(|e| e.to_string())?.ilst().cloned().unwrap_or_default();
            let tempo = (bpm.round_ties_even() as i64).clamp(1, 65535) as u16;
            tag.insert(Atom::new(
                AtomIdent::Fourcc(*b"tmpo"),
                AtomData::Unknown { code: DataType::BeSignedInteger, data: tempo.to_be_bytes().to_vec() },
            ));
            tag.insert(Atom::new(
                AtomIdent::Freeform { mean: Cow::Borrowed("com.apple.iTunes"), name: Cow::Borrowed("initialkey") },
                AtomData::UTF8(key.clone()),
            ));
            tag.save_to_path(path, save).map_err(|e| e.to_string())
        }
        FileType::Flac => {
            // FLAC keeps pictures beside its comment block; saving the comments alone would drop the cover.
            let flac = FlacFile::read_from(&mut open(path)?, options).map_err(|e| e.to_string())?;
            let mut tag = flac.vorbis_comments().cloned().unwrap_or_default();
            for (picture, info) in flac.pictures() {
                tag.insert_picture(picture.clone(), Some(*info)).map_err(|e| e.to_string())?;
            }
            vorbis(tag)
        }
        FileType::Opus => vorbis(OpusFile::read_from(&mut open(path)?, options).map_err(|e| e.to_string())?.vorbis_comments().clone()),
        FileType::Vorbis => vorbis(VorbisFile::read_from(&mut open(path)?, options).map_err(|e| e.to_string())?.vorbis_comments().clone()),
        _ => Err("Unsupported audio container for BPM/key tags".into()),
    }
}

const ACID_ROOT_SET: u32 = 0x02;
const ACID_STRETCH: u32 = 0x04;

/// Add or replace a WAV `acid` chunk: tempo, beats in the file, 4/4 meter and the
/// root note (the analysed tonic as a MIDI note from C4 = 60 to B4).
pub fn write_acid(path: &Path, bpm: f64, key: &str) -> Result<(), String> {
    if !bpm.is_finite() || !(bpm > 0.0 && bpm <= 999.0) {
        return Err("bpm must be finite and positive".into());
    }
    let tonic = crate::notation::parse_key(key).ok().map(|(t, _)| t as u16);
    let mut stream = OpenOptions::new().read(true).write(true).open(path).map_err(|e| e.to_string())?;
    let mut header = [0u8; 12];
    if stream.read_exact(&mut header).is_err() || &header[..4] != b"RIFF" || &header[8..] != b"WAVE" {
        return Err("ACID metadata requires a RIFF/WAVE file".into());
    }
    let size = stream.seek(SeekFrom::End(0)).map_err(|e| e.to_string())?;
    let end = 8 + u32::from_le_bytes(header[4..8].try_into().unwrap()) as u64;
    if end < 12 || end > size {
        return Err("WAV file has an invalid or truncated RIFF size".into());
    }
    let (mut position, mut acid, mut rate, mut block, mut frames) = (12u64, None, None, None, None);
    while position + 8 <= end {
        stream.seek(SeekFrom::Start(position)).map_err(|e| e.to_string())?;
        let mut chunk = [0u8; 8];
        stream.read_exact(&mut chunk).map_err(|e| e.to_string())?;
        let length = u32::from_le_bytes(chunk[4..].try_into().unwrap()) as u64;
        let next = position + 8 + length + (length & 1);
        if next > end {
            return Err("WAV file has a truncated chunk".into());
        }
        match &chunk[..4] {
            b"fmt " => {
                if length < 16 {
                    return Err("WAV file has an invalid format chunk".into());
                }
                let mut fmt = [0u8; 14];
                stream.read_exact(&mut fmt).map_err(|e| e.to_string())?;
                rate = Some(u32::from_le_bytes(fmt[4..8].try_into().unwrap()));
                block = Some(u16::from_le_bytes(fmt[12..14].try_into().unwrap()));
            }
            b"data" => frames = Some(length),
            b"acid" if length == 24 => acid = Some(position),
            _ => {}
        }
        position = next;
    }
    if position != end {
        return Err("WAV file has an incomplete chunk header".into());
    }
    // Replacing an existing chunk is safe; appending must not truncate data
    // outside RIFF (for example a trailing legacy tag).
    if acid.is_none() && end != size {
        return Err("Cannot append ACID metadata with trailing data outside RIFF".into());
    }
    let (Some(rate), Some(block), Some(frames)) = (rate.filter(|r| *r > 0), block.filter(|b| *b > 0), frames) else {
        return Err("WAV file is missing its format or audio data".into());
    };
    let seconds = (frames / block as u64) as f64 / rate as f64;
    let flags = ACID_STRETCH | if tonic.is_some() { ACID_ROOT_SET } else { 0 };
    let beats = ((seconds * bpm / 60.0).round_ties_even() as u32).max(1);
    let mut chunk = Vec::with_capacity(32);
    chunk.extend_from_slice(b"acid");
    chunk.extend_from_slice(&24u32.to_le_bytes());
    chunk.extend_from_slice(&flags.to_le_bytes());
    chunk.extend_from_slice(&(60 + tonic.unwrap_or(0)).to_le_bytes());
    chunk.extend_from_slice(&0x8000u16.to_le_bytes());
    chunk.extend_from_slice(&0f32.to_le_bytes());
    chunk.extend_from_slice(&beats.to_le_bytes());
    chunk.extend_from_slice(&4u16.to_le_bytes()); // denominator
    chunk.extend_from_slice(&4u16.to_le_bytes()); // numerator
    chunk.extend_from_slice(&(bpm as f32).to_le_bytes());
    match acid {
        Some(offset) => {
            stream.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
            stream.write_all(&chunk).map_err(|e| e.to_string())?;
        }
        None => {
            // Append after the last complete chunk and grow the RIFF size.
            let riff = position + chunk.len() as u64 - 8;
            let riff = u32::try_from(riff).map_err(|_| "WAV file is too large for RIFF metadata".to_string())?;
            stream.seek(SeekFrom::Start(position)).map_err(|e| e.to_string())?;
            stream.write_all(&chunk).map_err(|e| e.to_string())?;
            stream.set_len(position + chunk.len() as u64).map_err(|e| e.to_string())?;
            stream.seek(SeekFrom::Start(4)).map_err(|e| e.to_string())?;
            stream.write_all(&riff.to_le_bytes()).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bpm_text_drops_whole_decimal() {
        assert_eq!(bpm_text(120.0), "120");
        assert_eq!(bpm_text(120.46), "120.5");
    }

    fn wav(path: &Path, seconds: u32) {
        let rate = 8000u32;
        let data = vec![0u8; (rate * seconds * 2) as usize];
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * 2).to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&data);
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn acid_chunk_appended_then_replaced() {
        let dir = std::env::temp_dir().join(format!("siphon-acid-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("loop.wav");
        wav(&path, 4);
        write_acid(&path, 120.0, "Am").unwrap();
        let first = std::fs::read(&path).unwrap();
        assert_eq!(u32::from_le_bytes(first[4..8].try_into().unwrap()) as usize, first.len() - 8);
        let acid = first.windows(4).position(|w| w == b"acid").unwrap();
        assert_eq!(u16::from_le_bytes(first[acid + 12..acid + 14].try_into().unwrap()), 69);
        assert_eq!(u32::from_le_bytes(first[acid + 20..acid + 24].try_into().unwrap()), 8);
        write_acid(&path, 90.0, "C").unwrap();
        let second = std::fs::read(&path).unwrap();
        assert_eq!(second.len(), first.len());
        assert_eq!(f32::from_le_bytes(second[acid + 28..acid + 32].try_into().unwrap()), 90.0);
        write_tags(&path, 120.5, "A minor").unwrap();
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn acid_rejects_malformed_chunks_without_changing_the_file() {
        let dir = std::env::temp_dir().join(format!("siphon-acid-invalid-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("loop.wav");
        wav(&path, 1);
        let valid = std::fs::read(&path).unwrap();
        let mut cases = Vec::new();
        let mut truncated = valid.clone();
        truncated.pop();
        cases.push(truncated);
        let mut oversized_data = valid.clone();
        oversized_data[40..44].copy_from_slice(&u32::MAX.to_le_bytes());
        cases.push(oversized_data);
        let mut short_format = valid.clone();
        short_format[16..20].copy_from_slice(&8u32.to_le_bytes());
        cases.push(short_format);
        let mut partial_header = valid.clone();
        partial_header.extend_from_slice(b"bad");
        let size = partial_header.len() as u32 - 8;
        partial_header[4..8].copy_from_slice(&size.to_le_bytes());
        cases.push(partial_header);
        let mut trailing_tag = valid;
        trailing_tag.extend_from_slice(b"TAGpreserve this metadata");
        cases.push(trailing_tag);
        for bytes in cases {
            std::fs::write(&path, &bytes).unwrap();
            assert!(write_acid(&path, 120.0, "Am").is_err());
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(&dir).unwrap();
    }
}
