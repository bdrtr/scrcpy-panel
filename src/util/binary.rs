use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use std::io::{self, Read, Write};

/// Read a big-endian u16 from a reader
pub fn read_u16be<R: Read>(r: &mut R) -> io::Result<u16> {
    r.read_u16::<BigEndian>()
}

/// Read a big-endian u32 from a reader
pub fn read_u32be<R: Read>(r: &mut R) -> io::Result<u32> {
    r.read_u32::<BigEndian>()
}

/// Read a big-endian u64 from a reader
pub fn read_u64be<R: Read>(r: &mut R) -> io::Result<u64> {
    r.read_u64::<BigEndian>()
}

/// Write a big-endian u16 to a writer
pub fn write_u16be<W: Write>(w: &mut W, val: u16) -> io::Result<()> {
    w.write_u16::<BigEndian>(val)
}

/// Write a big-endian u32 to a writer
pub fn write_u32be<W: Write>(w: &mut W, val: u32) -> io::Result<()> {
    w.write_u32::<BigEndian>(val)
}

/// Write a big-endian u64 to a writer
pub fn write_u64be<W: Write>(w: &mut W, val: u64) -> io::Result<()> {
    w.write_u64::<BigEndian>(val)
}
