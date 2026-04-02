use error::Result;
use serde;
use std::io;

/// An optional Read trait for advanced Bincode usage.
///
/// It is highly recommended to use bincode with `io::Read` or `&[u8]` before
/// implementing a custom `BincodeRead`.
///
/// The forward_read_* methods are necessary because some byte sources want
/// to pass a long-lived borrow to the visitor and others want to pass a
/// transient slice.
pub trait BincodeRead<'storage>: io::Read {
    /// Check that the next `length` bytes are a valid string and pass
    /// it on to the serde reader.
    fn forward_read_str<V>(&mut self, length: usize, visitor: V) -> Result<V::Value>
    where
        V: serde::de::Visitor<'storage>;

    /// Transfer ownership of the next `length` bytes to the caller.
    fn get_byte_buffer(&mut self, length: usize) -> Result<Vec<u8>>;

    /// Pass a slice of the next `length` bytes on to the serde reader.
    fn forward_read_bytes<V>(&mut self, length: usize, visitor: V) -> Result<V::Value>
    where
        V: serde::de::Visitor<'storage>;
}

/// A BincodeRead implementation for byte slices
pub struct SliceReader<'storage> {
    slice: &'storage [u8],
}

/// A BincodeRead implementation for `io::Read`ers
pub struct IoReader<R> {
    reader: R,
    temp_buffer: Vec<u8>,
}

impl<'storage> SliceReader<'storage> {
    /// Constructs a slice reader
    pub(crate) fn new(bytes: &'storage [u8]) -> SliceReader<'storage> {
        SliceReader { slice: bytes }
    }

    #[inline(always)]
    fn get_byte_slice(&mut self, length: usize) -> Result<&'storage [u8]> {
        if length > self.slice.len() {
            return Err(SliceReader::unexpected_eof());
        }
        let (read_slice, remaining) = self.slice.split_at(length);
        self.slice = remaining;
        Ok(read_slice)
    }

    pub(crate) fn is_finished(&self) -> bool {
        self.slice.is_empty()
    }
}

impl<R> IoReader<R> {
    /// Constructs an IoReadReader
    pub(crate) fn new(r: R) -> IoReader<R> {
        IoReader {
            reader: r,
            temp_buffer: vec![],
        }
    }
}

impl<'storage> io::Read for SliceReader<'storage> {
    #[inline(always)]
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.len() > self.slice.len() {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        let (read_slice, remaining) = self.slice.split_at(out.len());
        out.copy_from_slice(read_slice);
        self.slice = remaining;

        Ok(out.len())
    }

    #[inline(always)]
    fn read_exact(&mut self, out: &mut [u8]) -> io::Result<()> {
        self.read(out).map(|_| ())
    }
}

impl<R: io::Read> io::Read for IoReader<R> {
    #[inline(always)]
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        self.reader.read(out)
    }
    #[inline(always)]
    fn read_exact(&mut self, out: &mut [u8]) -> io::Result<()> {
        self.reader.read_exact(out)
    }
}

impl<'storage> SliceReader<'storage> {
    #[inline(always)]
    fn unexpected_eof() -> Box<::ErrorKind> {
        Box::new(::ErrorKind::Io(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "",
        )))
    }
}

impl<'storage> BincodeRead<'storage> for SliceReader<'storage> {
    #[inline(always)]
    fn forward_read_str<V>(&mut self, length: usize, visitor: V) -> Result<V::Value>
    where
        V: serde::de::Visitor<'storage>,
    {
        use ErrorKind;
        let string = match ::std::str::from_utf8(self.get_byte_slice(length)?) {
            Ok(s) => s,
            Err(e) => return Err(ErrorKind::InvalidUtf8Encoding(e).into()),
        };
        visitor.visit_borrowed_str(string)
    }

    #[inline(always)]
    fn get_byte_buffer(&mut self, length: usize) -> Result<Vec<u8>> {
        self.get_byte_slice(length).map(|x| x.to_vec())
    }

    #[inline(always)]
    fn forward_read_bytes<V>(&mut self, length: usize, visitor: V) -> Result<V::Value>
    where
        V: serde::de::Visitor<'storage>,
    {
        visitor.visit_borrowed_bytes(self.get_byte_slice(length)?)
    }
}

impl<R> IoReader<R>
where
    R: io::Read,
{
    fn fill_buffer(&mut self, length: usize) -> Result<()> {
        if let Some(to_reserve) = length.checked_sub(self.temp_buffer.capacity()) {
            self.temp_buffer.try_reserve(to_reserve).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::OutOfMemory,
                    "size of allocation exceeds available memory",
                )
            })?;
        }
        self.temp_buffer.resize(length, 0);

        self.reader.read_exact(&mut self.temp_buffer)?;

        Ok(())
    }
}

impl<'a, R> BincodeRead<'a> for IoReader<R>
where
    R: io::Read,
{
    fn forward_read_str<V>(&mut self, length: usize, visitor: V) -> Result<V::Value>
    where
        V: serde::de::Visitor<'a>,
    {
        self.fill_buffer(length)?;

        let string = match ::std::str::from_utf8(&self.temp_buffer[..]) {
            Ok(s) => s,
            Err(e) => return Err(::ErrorKind::InvalidUtf8Encoding(e).into()),
        };

        visitor.visit_str(string)
    }

    fn get_byte_buffer(&mut self, length: usize) -> Result<Vec<u8>> {
        self.fill_buffer(length)?;
        Ok(::std::mem::replace(&mut self.temp_buffer, Vec::new()))
    }

    fn forward_read_bytes<V>(&mut self, length: usize, visitor: V) -> Result<V::Value>
    where
        V: serde::de::Visitor<'a>,
    {
        self.fill_buffer(length)?;
        visitor.visit_bytes(&self.temp_buffer[..])
    }
}

#[cfg(test)]
mod test {
    use super::IoReader;

    #[test]
    fn test_fill_buffer() {
        let buffer = vec![0u8; 64];
        let mut reader = IoReader::new(buffer.as_slice());

        reader.fill_buffer(20).unwrap();
        assert_eq!(20, reader.temp_buffer.len());

        reader.fill_buffer(30).unwrap();
        assert_eq!(30, reader.temp_buffer.len());

        reader.fill_buffer(5).unwrap();
        assert_eq!(5, reader.temp_buffer.len());
    }

    #[test]
    fn test_lfs_pointer_ioreader_no_limit() {
        use std::io::Cursor;

        // A Git LFS pointer file that hasn't been pulled. When fed to bincode v1.3.3, bytes 8-15
        // ("https://") are interpreted as a length of
        // 3400000511170344040 (0x2f2f3a7370747468).
        const LFS_POINTER: &[u8] = b"version https://git-lfs.github.com/spec/v1\n\
            oid sha256:6911f0f7f1d0457b10df382cc1d7130410036917134b8302c889eef9bd488f65\n\
            size 124958";

        // Test IoReader without a size limit (the default for deserialize_from) and adversarial
        // input; this would previously attempt to allocate 3400000511170344040 bytes and fail with
        // an error we cannot catch.
        let result = crate::deserialize_from::<_, Vec<String>>(Cursor::new(LFS_POINTER));

        assert!(result.is_err());
    }
}
