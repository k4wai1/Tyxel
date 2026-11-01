// Sub-Agente C: codec del contenedor binario .txv. Header fijo de 32 bytes,
// I-Frames, P-Frames (Op::Skip/Repeat/Literal), lz4_flex y Seek Index Footer.

use crate::tyxel::Tyxel;
use std::fmt;
use std::io::Write;

pub const TXV_MAGIC: [u8; 8] = *b"TYXELVID";
pub const TXV_END_MAGIC: [u8; 8] = *b"TYXELEND";
pub const TXV_VERSION: u8 = 1;
pub const FRAME_TYPE_I: u8 = 0x01;
pub const FRAME_TYPE_P: u8 = 0x02;
pub const FOOTER_LEN: u64 = 16;
pub const HEADER_LEN: u64 = 32;

/// Modo de color base del archivo (bits 1-2 de `flags`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    Truecolor = 0,
    Ansi256 = 1,
    Ansi16 = 2,
    Mono = 3,
}

/// Compresión del payload (bits 3-4 de `flags`; v1.0 implementa None y LZ4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    None = 0,
    Lz4 = 1,
    Zstd = 2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TxvHeader {
    pub width: u16,
    pub height: u16,
    pub fps_num: u16,
    pub fps_den: u16,
    /// 0 = stream en vivo (sin contar).
    pub total_frames: u32,
    pub nerdfont: bool,
    pub color: ColorMode,
    pub compression: Compression,
    /// 0 = Atlas Lógico v1 (512 glifos).
    pub atlas_id: u16,
}

impl Default for TxvHeader {
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
            fps_num: 30,
            fps_den: 1,
            total_frames: 0,
            nerdfont: true,
            color: ColorMode::Truecolor,
            compression: Compression::Lz4,
            atlas_id: 0,
        }
    }
}

impl TxvHeader {
    pub fn to_bytes(&self) -> [u8; 32] {
        let mut b = [0u8; 32];
        b[0..8].copy_from_slice(&TXV_MAGIC);
        b[8] = TXV_VERSION;
        let mut flags = 0u8;
        if self.nerdfont {
            flags |= 1;
        }
        flags |= (self.color as u8) << 1;
        flags |= (self.compression as u8) << 3;
        b[9] = flags;
        b[10..12].copy_from_slice(&self.width.to_le_bytes());
        b[12..14].copy_from_slice(&self.height.to_le_bytes());
        b[14..16].copy_from_slice(&self.fps_num.to_le_bytes());
        b[16..18].copy_from_slice(&self.fps_den.to_le_bytes());
        b[18..22].copy_from_slice(&self.total_frames.to_le_bytes());
        b[22..24].copy_from_slice(&self.atlas_id.to_le_bytes());
        b
    }

    pub fn from_bytes(b: &[u8; 32]) -> Result<Self, TxvError> {
        if b[0..8] != TXV_MAGIC {
            return Err(TxvError::BadMagic);
        }
        if b[8] != TXV_VERSION {
            return Err(TxvError::BadVersion(b[8]));
        }
        let flags = b[9];
        let color = match (flags >> 1) & 0b11 {
            0 => ColorMode::Truecolor,
            1 => ColorMode::Ansi256,
            2 => ColorMode::Ansi16,
            _ => ColorMode::Mono,
        };
        let compression = match (flags >> 3) & 0b11 {
            0 => Compression::None,
            1 => Compression::Lz4,
            2 => Compression::Zstd,
            _ => return Err(TxvError::BadCompression),
        };
        Ok(Self {
            width: u16::from_le_bytes([b[10], b[11]]),
            height: u16::from_le_bytes([b[12], b[13]]),
            fps_num: u16::from_le_bytes([b[14], b[15]]),
            fps_den: u16::from_le_bytes([b[16], b[17]]),
            total_frames: u32::from_le_bytes([b[18], b[19], b[20], b[21]]),
            nerdfont: flags & 1 == 1,
            color,
            compression,
            atlas_id: u16::from_le_bytes([b[22], b[23]]),
        })
    }

    #[inline]
    pub fn cells_per_frame(&self) -> usize {
        self.width as usize * self.height as usize
    }
}

// ---------- Deltas ----------

/// Operación delta de un P-Frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// Salta N celdas idénticas al fotograma anterior.
    Skip(u16),
    /// Pinta el mismo Tyxel N veces (RLE de fondos).
    Repeat(u16, Tyxel),
    /// Copia N celdas distintas contiguas.
    Literal(Vec<Tyxel>),
}

/// Duplica el conteo N en trozos de hasta u16::MAX.
#[inline]
fn split_count(mut n: usize) -> Vec<u16> {
    let mut v = Vec::new();
    while n > 0 {
        let c = n.min(usize::from(u16::MAX));
        v.push(c as u16);
        n -= c;
    }
    v
}

/// Calcula la secuencia de ops que transforma `prev` en `cur` (mismo len).
pub fn delta_ops(prev: &[Tyxel], cur: &[Tyxel]) -> Vec<Op> {
    assert_eq!(prev.len(), cur.len(), "frames deben tener el mismo len");
    let mut ops = Vec::new();
    let mut i = 0usize;
    while i < cur.len() {
        if cur[i] == prev[i] {
            let mut n = 1usize;
            while i + n < cur.len() && cur[i + n] == prev[i + n] {
                n += 1;
            }
            for n in split_count(n) {
                ops.push(Op::Skip(n));
            }
            i += n;
            continue;
        }
        // Run de cambios: se extiende mientras haya cambios o la celda
        // siguiente sea idéntica a la última del segmento (fusionable con RLE).
        let mut seg: Vec<Tyxel> = Vec::new();
        while i < cur.len()
            && (cur[i] != prev[i] || (!seg.is_empty() && cur[i] == *seg.last().unwrap()))
        {
            seg.push(cur[i]);
            i += 1;
            if seg.len() >= usize::from(u16::MAX) {
                break;
            }
        }
        let mut j = 0usize;
        while j < seg.len() {
            let mut run = 1usize;
            while j + run < seg.len() && seg[j + run] == seg[j] {
                run += 1;
            }
            if run >= 2 {
                for n in split_count(run) {
                    ops.push(Op::Repeat(n, seg[j]));
                }
                j += run;
            } else {
                let lit_start = j;
                while j < seg.len() && (j + 1 >= seg.len() || seg[j + 1] != seg[j]) {
                    j += 1;
                }
                let _ = lit_start;
                ops.push(Op::Literal(seg[0..j].to_vec()));
            }
        }
    }
    ops
}

/// Aplica ops sobre `prev` para reproducir `cur`.
///
/// Interpreta los Skips copiando celdas del frame anterior y desprecia
/// silenciosamente restos corruptos (nunca pánicos en runtime).
pub fn apply_frame(prev: &[Tyxel], ops: &[Op]) -> Vec<Tyxel> {
    let mut out = Vec::with_capacity(prev.len());
    for op in ops {
        match op {
            Op::Skip(n) => {
                let n = usize::from(*n);
                let pos = out.len();
                if pos + n > prev.len() {
                    return out; // corrupto: devolver lo que hay
                }
                out.extend(prev[pos..pos + n].iter().copied());
            }
            Op::Repeat(n, t) => {
                for _ in 0..usize::from(*n) {
                    out.push(*t);
                }
            }
            Op::Literal(v) => out.extend(v.iter().copied()),
        }
    }
    out
}

// ---------- Errores ----------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TxvError {
    BadMagic,
    BadVersion(u8),
    BadCompression,
    BadFooter,
    Truncated,
    FrameLen { expect: usize, got: usize },
    BadOps,
}

impl fmt::Display for TxvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadMagic => write!(f, "magic TYXELVID ausente"),
            Self::BadVersion(v) => write!(f, "versión no soportada: {v}"),
            Self::BadCompression => write!(f, "modo de compresión inválido"),
            Self::BadFooter => write!(f, "footer TYXELEND corrupto"),
            Self::Truncated => write!(f, "archivo truncado"),
            Self::FrameLen { expect, got } => {
                write!(f, "frame con {got} celdas, esperaba {expect}")
            }
            Self::BadOps => write!(f, "ops delta inválidas"),
        }
    }
}

impl std::error::Error for TxvError {}

// ---------- Escalado de compresión (empaquetado de payloads) ----------

fn pack_payload(h: &TxvHeader, raw: &[u8]) -> Vec<u8> {
    match h.compression {
        Compression::None => raw.to_vec(),
        Compression::Lz4 => lz4_flex::compress_prepend_size(raw),
        Compression::Zstd => raw.to_vec(), // v2: vía CLI externa
    }
}

fn unpack_payload(h: &TxvHeader, payload: &[u8]) -> Result<Vec<u8>, TxvError> {
    match h.compression {
        Compression::None => Ok(payload.to_vec()),
        Compression::Lz4 => {
            lz4_flex::decompress_size_prepended(payload).map_err(|_| TxvError::BadOps)
        }
        Compression::Zstd => Ok(payload.to_vec()),
    }
}

/// Dura payload codifica ops (tag u8 + count u16 LE + datos).
pub fn encode_ops(ops: &[Op]) -> Vec<u8> {
    let mut out = Vec::new();
    for op in ops {
        match op {
            Op::Skip(n) => {
                out.push(0);
                out.extend(n.to_le_bytes());
            }
            Op::Repeat(n, t) => {
                out.push(1);
                out.extend(n.to_le_bytes());
                out.extend(t.bytes());
            }
            Op::Literal(v) => {
                out.push(2);
                out.extend((v.len() as u16).to_le_bytes());
                for t in v {
                    out.extend(t.bytes());
                }
            }
        }
    }
    out
}

pub fn decode_ops(data: &[u8]) -> Result<Vec<Op>, TxvError> {
    let mut ops = Vec::new();
    let mut i = 0usize;
    while i < data.len() {
        let tag = *data.get(i).ok_or(TxvError::Truncated)?;
        i += 1;
        let n = u16::from_le_bytes([
            *data.get(i).ok_or(TxvError::Truncated)?,
            *data.get(i + 1).ok_or(TxvError::Truncated)?,
        ]);
        i += 2;
        match tag {
            0 => ops.push(Op::Skip(n)),
            1 => {
                let bytes: [u8; 8] = data
                    .get(i..i + 8)
                    .ok_or(TxvError::Truncated)?
                    .try_into()
                    .map_err(|_| TxvError::BadOps)?;
                i += 8;
                ops.push(Op::Repeat(n, Tyxel::from_u64(u64::from_le_bytes(bytes))));
            }
            2 => {
                let cells = usize::from(n);
                let b = data.get(i..i + cells * 8).ok_or(TxvError::Truncated)?;
                i += cells * 8;
                let mut v = Vec::with_capacity(cells);
                for c in b.as_chunks::<8>().0 {
                    let g = u64::from_le_bytes(*c);
                    v.push(Tyxel::from_u64(g));
                }
                ops.push(Op::Literal(v));
            }
            _ => return Err(TxvError::BadOps),
        }
    }
    Ok(ops)
}

// ---------- Escritura ----------

/// Escritor de un solo pase para .txv, con Seek Index al cerrar.
pub struct TxvWriter<W: Write> {
    w: W,
    header: TxvHeader,
    pos: u64,
    index: Vec<(u32, u64)>, // (timestamp_ms, offset absoluto del packet)
    clock_ms: u32,
    frames: u32,
}

impl<W: Write> TxvWriter<W> {
    pub fn new(mut w: W, header: TxvHeader) -> std::io::Result<Self> {
        let mut buf = header.to_bytes();
        if header.total_frames > 0 {
            buf[18..22].copy_from_slice(&header.total_frames.to_le_bytes());
        }
        w.write_all(&buf)?;
        Ok(Self {
            pos: HEADER_LEN,
            header,
            w,
            index: Vec::new(),
            clock_ms: 0,
            frames: 0,
        })
    }

    /// Empaqueta un fotograma (0x01 I-Frame or 0x02 P-Frame) con duration_ms.
    fn write_packet(
        &mut self,
        frame_type: u8,
        duration_ms: u16,
        payload: &[u8],
    ) -> std::io::Result<()> {
        let packed = pack_payload(&self.header, payload);
        let packet_start = self.pos;
        self.w.write_all(&[frame_type])?;
        self.w.write_all(&duration_ms.to_le_bytes())?;
        self.w.write_all(&(packed.len() as u32).to_le_bytes())?;
        self.w.write_all(&packed)?;
        self.pos += 1 + 2 + 4 + packed.len() as u64;
        if frame_type == FRAME_TYPE_I {
            self.index.push((self.clock_ms, packet_start));
        }
        self.clock_ms = self.clock_ms.saturating_add(u32::from(duration_ms));
        self.frames += 1;
        Ok(())
    }

    pub fn push_iframe(&mut self, cells: &[Tyxel], duration_ms: u16) -> std::io::Result<()> {
        debug_assert_eq!(cells.len(), self.header.cells_per_frame());
        let mut raw = Vec::with_capacity(cells.len() * 8);
        for t in cells {
            let v = t.as_u64();
            raw.extend(v.to_le_bytes());
        }
        self.write_packet(FRAME_TYPE_I, duration_ms, &raw)
    }

    pub fn push_pframe(
        &mut self,
        prev: &[Tyxel],
        cur: &[Tyxel],
        duration_ms: u16,
    ) -> std::io::Result<()> {
        debug_assert_eq!(cur.len(), self.header.cells_per_frame());
        let ops = delta_ops(prev, cur);
        let raw = encode_ops(&ops);
        self.write_packet(FRAME_TYPE_P, duration_ms, &raw)
    }

    /// Cierra: vuelca el Seek Index Trailer (count u32 + (ts u32, off u64)*n)
    /// y el footer fijo de 16 bytes (index_offset + TYXELEND).
    pub fn finish(mut self) -> std::io::Result<(u32, Vec<(u32, u64)>)> {
        let index_offset = self.pos;
        let mut trailer = Vec::new();
        trailer.extend((self.index.len() as u32).to_le_bytes());
        for (ts, off) in self.index.clone() {
            trailer.extend(ts.to_le_bytes());
            trailer.extend(off.to_le_bytes());
        }
        self.w.write_all(&trailer)?;
        self.w.write_all(&index_offset.to_le_bytes())?;
        self.w.write_all(&TXV_END_MAGIC)?;
        self.w.flush()?;
        Ok((self.frames, self.index))
    }
}

// ---------- Lectura (principalmente para tests y `tyxel info`) ----------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxvFrame {
    pub frame_type: u8,
    pub duration_ms: u16,
    pub cells: Vec<Tyxel>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Txv {
    pub header: TxvHeader,
    pub frames: Vec<TxvFrame>,
    pub index: Vec<(u32, u64)>,
}

impl Txv {
    /// Parsea la totalidad de un buffer .txv y decodifica frames en RAM.
    pub fn parse(data: &[u8]) -> Result<Self, TxvError> {
        if data.len() < 48 {
            return Err(TxvError::Truncated);
        }
        let mut hb = [0u8; 32];
        hb.copy_from_slice(&data[..32]);
        let header = TxvHeader::from_bytes(&hb)?;
        let n_cells = header.cells_per_frame();
        // Footer: [index_offset u64][TYXELEND]
        if data.len() < usize::try_from(FOOTER_LEN).unwrap() {
            return Err(TxvError::Truncated);
        }
        let f_start = data.len() - usize::try_from(FOOTER_LEN).unwrap();
        let end_magic: [u8; 8] = data[f_start + 8..f_start + 16]
            .try_into()
            .map_err(|_| TxvError::BadFooter)?;
        if end_magic != TXV_END_MAGIC {
            return Err(TxvError::BadFooter);
        }
        let index_offset = u64::from_le_bytes(
            data[f_start..f_start + 8]
                .try_into()
                .map_err(|_| TxvError::BadFooter)?,
        );
        // Seek Index Trailer
        let mut index: Vec<(u32, u64)> = Vec::new();
        if index_offset > 0 {
            let idx_start = usize::try_from(index_offset).map_err(|_| TxvError::BadFooter)?;
            if idx_start + 4 > f_start {
                return Err(TxvError::BadFooter);
            }
            let cnt = u32::from_le_bytes(
                data[idx_start..idx_start + 4]
                    .try_into()
                    .map_err(|_| TxvError::BadFooter)?,
            ) as usize;
            let need = cnt * 12;
            if idx_start + 4 + need > f_start {
                return Err(TxvError::BadFooter);
            }
            let mut i = idx_start + 4;
            for _ in 0..cnt {
                let ts =
                    u32::from_le_bytes(data[i..i + 4].try_into().map_err(|_| TxvError::BadFooter)?);
                let off = u64::from_le_bytes(
                    data[i + 4..i + 12]
                        .try_into()
                        .map_err(|_| TxvError::BadFooter)?,
                );
                index.push((ts, off));
                i += 12;
            }
        }
        // Stream de packets hasta index_offset (o f_start).
        let stream_end = if index_offset > 0 {
            usize::try_from(index_offset).unwrap()
        } else {
            f_start
        };
        let mut frames: Vec<TxvFrame> = Vec::new();
        let mut prev: Vec<Tyxel> = vec![Tyxel::default(); n_cells];
        let mut cursor = HEADER_LEN as usize;
        while cursor < stream_end {
            let ft = *data.get(cursor).ok_or(TxvError::Truncated)?;
            let rd = cursor + 1;
            let dur = u16::from_le_bytes(
                data.get(rd..rd + 2)
                    .ok_or(TxvError::Truncated)?
                    .try_into()
                    .map_err(|_| TxvError::BadFooter)?,
            );
            let len = u32::from_le_bytes(
                data.get(rd + 2..rd + 6)
                    .ok_or(TxvError::Truncated)?
                    .try_into()
                    .map_err(|_| TxvError::BadFooter)?,
            ) as usize;
            let payload = data.get(rd + 6..rd + 6 + len).ok_or(TxvError::Truncated)?;
            cursor = rd + 6 + len;
            let raw = unpack_payload(&header, payload)?;
            let cells = if ft == FRAME_TYPE_I {
                let mut v = Vec::with_capacity(n_cells);
                for c in raw.as_chunks::<8>().0 {
                    v.push(Tyxel::from_u64(u64::from_le_bytes(*c)));
                }
                if v.len() != n_cells {
                    return Err(TxvError::FrameLen {
                        expect: n_cells,
                        got: v.len(),
                    });
                }
                v
            } else if ft == FRAME_TYPE_P {
                let ops = decode_ops(&raw)?;
                let mut v = apply_frame(&prev, &ops);
                v.truncate(n_cells); // defensa: decodificadores ligeros
                if v.len() != n_cells {
                    return Err(TxvError::FrameLen {
                        expect: n_cells,
                        got: v.len(),
                    });
                }
                v
            } else {
                return Err(TxvError::BadOps);
            };
            prev = cells.clone();
            frames.push(TxvFrame {
                frame_type: ft,
                duration_ms: dur,
                cells,
            });
        }
        Ok(Self {
            header,
            frames,
            index,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atlas;

    fn cell(g: char, fg: u8) -> Tyxel {
        Tyxel {
            glyph_id: atlas::id_of_codepoint(g).expect("glifo en atlas"),
            fg: [fg; 3],
            bg: [0; 3],
        }
    }

    fn header() -> TxvHeader {
        TxvHeader {
            width: 2,
            height: 2,
            ..TxvHeader::default()
        }
    }

    fn sample_frames() -> Vec<Vec<Tyxel>> {
        vec![
            vec![
                cell('█', 0xAA),
                cell(' ', 0x22),
                cell('▀', 0x00),
                cell('█', 0xFF),
            ],
            vec![
                cell('█', 0xAA),
                cell(' ', 0x22),
                cell('▀', 0x00),
                cell(' ', 0x00),
            ],
            vec![
                cell('█', 0xAA),
                cell(' ', 0x22),
                cell('▀', 0x00),
                cell(' ', 0x00),
            ],
        ]
    }

    #[test]
    fn delta_ops_round_trip() {
        let frames = sample_frames();
        let ops = delta_ops(&frames[0], &frames[1]);
        let rebuilt = apply_frame(&frames[0], &ops);
        assert_eq!(rebuilt, frames[1]);
        // identicos -> solo skip
        let ops2 = delta_ops(&frames[1], &frames[2]);
        assert!(ops2.iter().all(|o| matches!(o, Op::Skip(_))));
        assert_eq!(apply_frame(&frames[1], &ops2), frames[2]);
    }

    #[test]
    fn ops_encode_round_trip() {
        let ops = vec![
            Op::Skip(3),
            Op::Repeat(2, cell('█', 0x11)),
            Op::Literal(vec![cell('▀', 0x22), cell(' ', 0x33)]),
        ];
        let decoded = decode_ops(&encode_ops(&ops)).unwrap();
        assert_eq!(decoded, ops);
    }

    #[test]
    fn txv_round_trip_lz4() {
        let h = header();
        let frames = sample_frames();
        let mut buf = Vec::new();
        let mut w = TxvWriter::new(&mut buf, h).unwrap();
        w.push_iframe(&frames[0], 100).unwrap();
        w.push_pframe(&frames[0], &frames[1], 33).unwrap();
        w.push_pframe(&frames[1], &frames[2], 100).unwrap();
        let finish = w.finish().unwrap();
        assert_eq!(finish.0, 3);
        let doc = Txv::parse(&buf).unwrap();
        assert_eq!(doc.header, h);
        assert_eq!(doc.frames.len(), 3);
        for (i, f) in doc.frames.iter().enumerate() {
            assert_eq!(f.cells, frames[i], "frame {i}");
        }
        assert_eq!(doc.frames[0].duration_ms, 100);
        assert_eq!(doc.frames[1].duration_ms, 33);
        assert_eq!(doc.index.len(), 1, "solo I-Frame se indexa");
        let (ts, off) = doc.index[0];
        assert_eq!(ts, 0);
        assert_eq!(doc.frames[0].frame_type, FRAME_TYPE_I);
        let _ = off;
    }

    #[test]
    fn txv_round_trip_no_compression() {
        let h = TxvHeader {
            compression: Compression::None,
            ..header()
        };
        let frames = sample_frames();
        let mut buf = Vec::new();
        let mut w = TxvWriter::new(&mut buf, h).unwrap();
        w.push_iframe(&frames[0], 50).unwrap();
        w.push_pframe(&frames[0], &frames[1], 50).unwrap();
        let _ = w.finish().unwrap();
        let doc = Txv::parse(&buf).unwrap();
        assert_eq!(doc.frames[1].cells, frames[1]);
        assert_eq!(doc.header.compression, Compression::None);
    }

    #[test]
    fn bad_footer_rejected() {
        let h = header();
        let frames = sample_frames();
        let mut buf = Vec::new();
        let mut w = TxvWriter::new(&mut buf, h).unwrap();
        w.push_iframe(&frames[0], 50).unwrap();
        let _ = w.finish().unwrap();
        let n = buf.len();
        buf[n - 1] ^= 1;
        assert!(matches!(Txv::parse(&buf), Err(TxvError::BadFooter)));
    }

    #[test]
    fn seek_index_offsets_resolve() {
        let h = header();
        let frames = sample_frames();
        let mut buf = Vec::new();
        let mut w = TxvWriter::new(&mut buf, h).unwrap();
        w.push_iframe(&frames[0], 50).unwrap();
        w.push_pframe(&frames[0], &frames[1], 16).unwrap();
        let (frames_n, index) = w.finish().unwrap();
        assert_eq!(frames_n, 2);
        for (_ts, off) in index {
            let ft = buf[off as usize];
            assert!(ft == FRAME_TYPE_I);
        }
    }
}
