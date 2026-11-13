//! Reads when a photo was taken from its EXIF block (only ever inside the
//! sandboxed decoder worker). Strictly bounded: a fixed number of entries
//! is looked at, every offset is checked, and anything odd gives `None`.

/// Most entries examined in one directory (real files have a few dozen).
const MAX_ENTRIES: u16 = 512;
const TAG_EXIF_IFD: u16 = 0x8769;
const TAG_DATE_TIME_ORIGINAL: u16 = 0x9003;
const TAG_DATE_TIME: u16 = 0x0132;
const TYPE_ASCII: u16 = 2;
const TYPE_LONG: u16 = 4;

struct Tiff<'a> {
    data: &'a [u8],
    little: bool,
}

impl Tiff<'_> {
    fn u16_at(&self, at: usize) -> Option<u16> {
        let b: [u8; 2] = self.data.get(at..at.checked_add(2)?)?.try_into().ok()?;
        Some(if self.little {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    }

    fn u32_at(&self, at: usize) -> Option<u32> {
        let b: [u8; 4] = self.data.get(at..at.checked_add(4)?)?.try_into().ok()?;
        Some(if self.little {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    }

    /// The (type, count, value-field offset) of `tag` in the directory at
    /// `ifd`.
    fn find(&self, ifd: usize, tag: u16) -> Option<(u16, u32, usize)> {
        let count = self.u16_at(ifd)?.min(MAX_ENTRIES);
        for i in 0..usize::from(count) {
            let entry = ifd.checked_add(2)?.checked_add(i.checked_mul(12)?)?;
            if self.u16_at(entry)? == tag {
                let kind = self.u16_at(entry.checked_add(2)?)?;
                let n = self.u32_at(entry.checked_add(4)?)?;
                return Some((kind, n, entry.checked_add(8)?));
            }
        }
        None
    }

    /// An ASCII date value ("YYYY:MM:DD HH:MM:SS").
    fn date(&self, ifd: usize, tag: u16) -> Option<u64> {
        let (kind, n, field) = self.find(ifd, tag)?;
        if kind != TYPE_ASCII || !(19..=32).contains(&n) {
            return None;
        }
        // Values longer than 4 bytes live at the offset stored in the field.
        let at = usize::try_from(self.u32_at(field)?).ok()?;
        parse_date(self.data.get(at..at.checked_add(19)?)?)
    }
}

/// `YYYY:MM:DD HH:MM:SS` → `YYYYMMDDHHMMSS`, or `None` if it isn't a real
/// date (cameras write all zeros or spaces when unset).
#[must_use]
pub fn parse_date(s: &[u8]) -> Option<u64> {
    let s: &[u8; 19] = s.get(..19)?.try_into().ok()?;
    let seps = [(4, b':'), (7, b':'), (10, b' '), (13, b':'), (16, b':')];
    if seps.iter().any(|&(i, c)| s.get(i) != Some(&c)) {
        return None;
    }
    let num = |a: usize, b: usize| -> Option<u64> {
        s.get(a..b)?.iter().try_fold(0u64, |acc, d| {
            d.is_ascii_digit().then(|| acc * 10 + u64::from(d - b'0'))
        })
    };
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (h, mi, se) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    from_parts(y, mo, d, h, mi, se)
}

fn from_parts(y: u64, mo: u64, d: u64, h: u64, mi: u64, se: u64) -> Option<u64> {
    let valid = (1900..=2200).contains(&y)
        && (1..=12).contains(&mo)
        && (1..=31).contains(&d)
        && h < 24
        && mi < 60
        && se < 61;
    valid.then(|| ((((y * 100 + mo) * 100 + d) * 100 + h) * 100 + mi) * 100 + se)
}

/// Whether `v` is a value [`taken`] could return.
#[must_use]
pub fn is_valid_taken(v: u64) -> bool {
    let part = |div: u64, modulo: u64| (v / div) % modulo;
    from_parts(
        v / 10_000_000_000,
        part(100_000_000, 100),
        part(1_000_000, 100),
        part(10_000, 100),
        part(100, 100),
        v % 100,
    ) == Some(v)
}

/// When the photo was taken, as `YYYYMMDDHHMMSS`, from a raw EXIF chunk
/// (starting with the TIFF header). Prefers the original capture time.
#[must_use]
pub fn taken(chunk: &[u8]) -> Option<u64> {
    let little = match chunk.get(..4)? {
        [0x49, 0x49, 42, 0] => true,
        [0x4d, 0x4d, 0, 42] => false,
        _ => return None,
    };
    let t = Tiff {
        data: chunk,
        little,
    };
    let ifd0 = usize::try_from(t.u32_at(4)?).ok()?;
    let from_exif_ifd = t.find(ifd0, TAG_EXIF_IFD).and_then(|(kind, n, field)| {
        if kind != TYPE_LONG || n != 1 {
            return None;
        }
        let sub = usize::try_from(t.u32_at(field)?).ok()?;
        t.date(sub, TAG_DATE_TIME_ORIGINAL)
    });
    from_exif_ifd.or_else(|| t.date(ifd0, TAG_DATE_TIME))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A little-endian EXIF chunk with IFD0 → Exif IFD → DateTimeOriginal.
    fn chunk(original: &[u8], ifd0_date: Option<&[u8]>) -> Vec<u8> {
        let mut c = b"II*\0".to_vec();
        c.extend_from_slice(&8u32.to_le_bytes());
        // IFD0 at 8: entries.
        let n0: u16 = if ifd0_date.is_some() { 2 } else { 1 };
        c.extend_from_slice(&n0.to_le_bytes());
        let ifd0_end = 8 + 2 + 12 * usize::from(n0) + 4;
        let exif_ifd = ifd0_end;
        let exif_end = exif_ifd + 2 + 12 + 4;
        let date1 = exif_end;
        let date2 = date1 + 20;
        let entry = |c: &mut Vec<u8>, tag: u16, kind: u16, n: u32, v: u32| {
            c.extend_from_slice(&tag.to_le_bytes());
            c.extend_from_slice(&kind.to_le_bytes());
            c.extend_from_slice(&n.to_le_bytes());
            c.extend_from_slice(&v.to_le_bytes());
        };
        if ifd0_date.is_some() {
            entry(&mut c, TAG_DATE_TIME, 2, 20, date2 as u32);
        }
        entry(&mut c, TAG_EXIF_IFD, 4, 1, exif_ifd as u32);
        c.extend_from_slice(&0u32.to_le_bytes());
        c.extend_from_slice(&1u16.to_le_bytes());
        entry(&mut c, TAG_DATE_TIME_ORIGINAL, 2, 20, date1 as u32);
        c.extend_from_slice(&0u32.to_le_bytes());
        c.extend_from_slice(original);
        c.push(0);
        if let Some(d) = ifd0_date {
            c.extend_from_slice(d);
            c.push(0);
        }
        c
    }

    #[test]
    fn reads_the_capture_time() {
        assert_eq!(
            taken(&chunk(b"2024:03:15 12:30:45", None)),
            Some(20240315123045)
        );
        // Falls back to the file's DateTime when the original is unset.
        assert_eq!(
            taken(&chunk(b"0000:00:00 00:00:00", Some(b"2019:12:31 23:59:59"))),
            Some(20191231235959)
        );
        assert!(is_valid_taken(20240315123045));
        assert!(!is_valid_taken(0) && !is_valid_taken(20241315000000));
    }

    #[test]
    fn rejects_junk_without_panicking() {
        assert_eq!(taken(b""), None);
        assert_eq!(taken(b"MM\0*\xff\xff\xff\xff"), None);
        assert_eq!(parse_date(b"2024-03-15 12:30:45"), None);
        assert_eq!(parse_date(b"    :  :     :  :  "), None);
        // Every truncation of a valid chunk is safe.
        let good = chunk(b"2024:03:15 12:30:45", Some(b"2019:12:31 23:59:59"));
        for len in 0..good.len() {
            let _ = taken(&good[..len]);
        }
        // Every single-byte corruption is safe.
        for i in 0..good.len() {
            let mut bad = good.clone();
            bad[i] ^= 0xff;
            let _ = taken(&bad);
        }
    }
}
