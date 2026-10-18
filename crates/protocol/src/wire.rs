//! Big-endian, length-prefixed field encoding shared by the binary Data
//! channel messages (file transfer and tunnels).

pub(crate) fn put_u16(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_be_bytes());
}

pub(crate) fn put_u64(bytes: &mut Vec<u8>, value: u64) {
    bytes.extend_from_slice(&value.to_be_bytes());
}

pub(crate) fn put_bytes(bytes: &mut Vec<u8>, value: &[u8]) {
    let length = u32::try_from(value.len()).expect("Data message field exceeds u32 length");
    bytes.extend_from_slice(&length.to_be_bytes());
    bytes.extend_from_slice(value);
}

pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn take(&mut self, length: usize) -> Option<&'a [u8]> {
        let end = self.position.checked_add(length)?;
        let value = self.bytes.get(self.position..end)?;
        self.position = end;
        Some(value)
    }

    pub(crate) fn take_u8(&mut self) -> Option<u8> {
        Some(*self.take(1)?.first()?)
    }

    pub(crate) fn take_u16(&mut self) -> Option<u16> {
        Some(u16::from_be_bytes(self.take(2)?.try_into().ok()?))
    }

    fn take_u32(&mut self) -> Option<u32> {
        Some(u32::from_be_bytes(self.take(4)?.try_into().ok()?))
    }

    pub(crate) fn take_u64(&mut self) -> Option<u64> {
        Some(u64::from_be_bytes(self.take(8)?.try_into().ok()?))
    }

    pub(crate) fn take_bytes(&mut self) -> Option<Vec<u8>> {
        let length = usize::try_from(self.take_u32()?).ok()?;
        Some(self.take(length)?.to_vec())
    }

    pub(crate) fn take_string(&mut self) -> Option<String> {
        String::from_utf8(self.take_bytes()?).ok()
    }

    pub(crate) fn take_array<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.take(N)?.try_into().ok()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.position == self.bytes.len()
    }
}
