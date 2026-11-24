//! A read-only, bounds-checked reader for the Compound File Binary format ([MS-CFB]) that holds
//! an OLE object's streams. It reads what a stream's sector chain reaches and stops at the end of
//! the data: AutoCAD stores the compound file without the unused tail of its last sector, so a
//! short last sector is read as far as it goes instead of being dropped.

const MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
/// Sector numbers from here up are markers (DIFAT, FAT, end of chain, free), not sectors.
const MAX_REGULAR: u32 = 0xFFFF_FFFA;
/// A directory entry that is a stream.
const STREAM: u8 = 2;
/// The most directory entries read (an OLE presentation has a handful).
const MAX_ENTRIES: usize = 4096;

/// A stream of the compound file: its name and its bytes.
pub(crate) struct Stream {
    pub name: String,
    pub data: Vec<u8>,
}

fn u16_at(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(o..o.checked_add(2)?)?.try_into().ok()?))
}

fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(o..o.checked_add(4)?)?.try_into().ok()?))
}

/// Little-endian 32-bit words of `b` (a partial last word left out).
fn words(b: &[u8]) -> Vec<u32> {
    b.as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c)).collect()
}

struct Reader<'a> {
    data: &'a [u8],
    sector: usize,
    fat: Vec<u32>,
}

impl Reader<'_> {
    /// Sector `i` (numbered from the one after the header), cut short at the end of the data.
    fn sector(&self, i: u32) -> Option<&[u8]> {
        let start = (i as usize).checked_add(1)?.checked_mul(self.sector)?;
        let end = start.checked_add(self.sector)?.min(self.data.len());
        self.data.get(start..end).filter(|s| !s.is_empty())
    }

    /// The bytes of the chain starting at `start` in `table`, each link read by `read`, up to
    /// `limit` bytes. A cycle, a link out of the table or a sector past the data ends it.
    fn chain<'b>(start: u32, table: &[u32], limit: usize, read: impl Fn(u32) -> Option<&'b [u8]>) -> Vec<u8> {
        let mut out = Vec::new();
        let mut cur = start;
        let mut seen = std::collections::HashSet::new();
        while cur < MAX_REGULAR && out.len() < limit && seen.insert(cur) {
            let Some(bytes) = read(cur) else { break };
            out.extend_from_slice(bytes);
            cur = table.get(cur as usize).copied().unwrap_or(u32::MAX);
        }
        out.truncate(limit);
        out
    }

    fn regular(&self, start: u32, limit: usize) -> Vec<u8> {
        Self::chain(start, &self.fat, limit, |i| self.sector(i))
    }
}

/// The streams of the compound file at the start of `data` (any storage, flattened), or `None`
/// when it is not one. Streams whose chains are broken come back as far as they could be read.
pub(crate) fn streams(data: &[u8]) -> Option<Vec<Stream>> {
    if data.get(..8)? != MAGIC {
        return None;
    }
    let shift = u16_at(data, 30)?;
    let mini_shift = u16_at(data, 32)?;
    if !(9..=12).contains(&shift) || !(6..shift).contains(&mini_shift) {
        return None;
    }
    let sector = 1usize << shift;
    let mini = 1usize << mini_shift;
    // The most sectors the data can hold: bounds every table and chain.
    let max_sectors = data.len() / sector + 1;
    let fat_count = (u32_at(data, 44)? as usize).min(max_sectors);
    let dir_start = u32_at(data, 48)?;
    let cutoff = u32_at(data, 56)? as usize;
    let mini_fat_start = u32_at(data, 60)?;
    let mut difat_next = u32_at(data, 68)?;
    // The FAT sectors: 109 in the header, the rest in the DIFAT chain.
    let mut fat_sectors: Vec<u32> = (0..109).filter_map(|k| u32_at(data, 76 + k * 4)).filter(|s| *s < MAX_REGULAR).collect();
    let mut r = Reader { data, sector, fat: Vec::new() };
    let mut seen = std::collections::HashSet::new();
    while difat_next < MAX_REGULAR && fat_sectors.len() < fat_count && seen.insert(difat_next) {
        let Some(s) = r.sector(difat_next) else { break };
        let n = sector / 4 - 1;
        fat_sectors.extend((0..n).filter_map(|k| u32_at(s, k * 4)).filter(|s| *s < MAX_REGULAR));
        difat_next = u32_at(s, n * 4).unwrap_or(u32::MAX);
    }
    fat_sectors.truncate(fat_count.max(1));
    let mut fat = Vec::new();
    for s in fat_sectors {
        if let Some(b) = r.sector(s) {
            fat.extend(words(b));
        }
    }
    r.fat = fat;
    let dir = r.regular(dir_start, MAX_ENTRIES * 128);
    let entries: Vec<&[u8]> = dir.as_chunks::<128>().0.iter().map(|e| e.as_slice()).collect();
    let root = entries.first()?;
    let root_start = u32_at(root, 116)?;
    let root_size = u32_at(root, 120)? as usize;
    let mini_stream = r.regular(root_start, root_size.min(data.len()));
    let mini_fat_bytes = r.regular(mini_fat_start, data.len());
    let mini_fat = words(&mini_fat_bytes);
    let mut out = Vec::new();
    for e in entries.iter().skip(1) {
        if e.get(66) != Some(&STREAM) {
            continue;
        }
        let name_len = usize::from(u16_at(e, 64).unwrap_or(0)).min(64);
        let units: Vec<u16> =
            e.get(..name_len.saturating_sub(2)).unwrap_or_default().as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
        let name = String::from_utf16_lossy(&units);
        let (Some(start), Some(size)) = (u32_at(e, 116), u32_at(e, 120)) else { continue };
        let size = (size as usize).min(data.len());
        let bytes = if size < cutoff {
            Reader::chain(start, &mini_fat, size, |i| {
                let o = (i as usize).checked_mul(mini)?;
                let end = o.checked_add(mini)?.min(mini_stream.len());
                mini_stream.get(o..end).filter(|s| !s.is_empty())
            })
        } else {
            r.regular(start, size)
        };
        out.push(Stream { name, data: bytes });
    }
    Some(out)
}
