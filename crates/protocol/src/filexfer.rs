use crate::wire::{put_bytes, put_u64, Reader};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileMsg {
    Offer {
        id: u64,
        name: String,
        size: u64,
        hash: [u8; 32],
    },
    Accept {
        id: u64,
    },
    Reject {
        id: u64,
        reason: String,
    },
    Chunk {
        id: u64,
        offset: u64,
        data: Vec<u8>,
    },
    Done {
        id: u64,
    },
    Error {
        id: u64,
        message: String,
    },
    /// Receiver -> sender flow control: every byte below `offset` has been
    /// written. The sender keeps a bounded window un-acknowledged, and a
    /// final `Ack` at the full size confirms the verified file.
    Ack {
        id: u64,
        offset: u64,
    },
}

impl FileMsg {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        match self {
            Self::Offer {
                id,
                name,
                size,
                hash,
            } => {
                bytes.push(1);
                put_u64(&mut bytes, *id);
                put_bytes(&mut bytes, name.as_bytes());
                put_u64(&mut bytes, *size);
                bytes.extend_from_slice(hash);
            }
            Self::Accept { id } => {
                bytes.push(2);
                put_u64(&mut bytes, *id);
            }
            Self::Reject { id, reason } => {
                bytes.push(3);
                put_u64(&mut bytes, *id);
                put_bytes(&mut bytes, reason.as_bytes());
            }
            Self::Chunk { id, offset, data } => {
                bytes.push(4);
                put_u64(&mut bytes, *id);
                put_u64(&mut bytes, *offset);
                put_bytes(&mut bytes, data);
            }
            Self::Done { id } => {
                bytes.push(5);
                put_u64(&mut bytes, *id);
            }
            Self::Error { id, message } => {
                bytes.push(6);
                put_u64(&mut bytes, *id);
                put_bytes(&mut bytes, message.as_bytes());
            }
            Self::Ack { id, offset } => {
                bytes.push(7);
                put_u64(&mut bytes, *id);
                put_u64(&mut bytes, *offset);
            }
        }
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<FileMsg> {
        let mut reader = Reader::new(bytes);
        let variant = reader.take_u8()?;
        let message = match variant {
            1 => {
                let id = reader.take_u64()?;
                let name = reader.take_string()?;
                let size = reader.take_u64()?;
                let hash = reader.take_array::<32>()?;
                Self::Offer {
                    id,
                    name,
                    size,
                    hash,
                }
            }
            2 => Self::Accept {
                id: reader.take_u64()?,
            },
            3 => Self::Reject {
                id: reader.take_u64()?,
                reason: reader.take_string()?,
            },
            4 => Self::Chunk {
                id: reader.take_u64()?,
                offset: reader.take_u64()?,
                data: reader.take_bytes()?,
            },
            5 => Self::Done {
                id: reader.take_u64()?,
            },
            6 => Self::Error {
                id: reader.take_u64()?,
                message: reader.take_string()?,
            },
            7 => Self::Ack {
                id: reader.take_u64()?,
                offset: reader.take_u64()?,
            },
            _ => return None,
        };

        reader.is_empty().then_some(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(message: FileMsg) {
        let bytes = message.to_bytes();
        assert_eq!(FileMsg::from_bytes(&bytes), Some(message));
    }

    #[test]
    fn offer_roundtrips_with_unicode_filename_and_hash() {
        round_trip(FileMsg::Offer {
            id: 7,
            name: "résumé 📄.pdf".into(),
            size: 123_456,
            hash: [0xAB; 32],
        });
    }

    #[test]
    fn accept_roundtrips() {
        round_trip(FileMsg::Accept { id: 8 });
    }

    #[test]
    fn reject_roundtrips() {
        round_trip(FileMsg::Reject {
            id: 9,
            reason: "not now".into(),
        });
    }

    #[test]
    fn chunk_roundtrips_with_binary_data_and_offset() {
        round_trip(FileMsg::Chunk {
            id: 10,
            offset: 4_096,
            data: vec![0, 255, 1, 254, 2, 253],
        });
    }

    #[test]
    fn done_roundtrips() {
        round_trip(FileMsg::Done { id: 11 });
    }

    #[test]
    fn error_roundtrips() {
        round_trip(FileMsg::Error {
            id: 12,
            message: "hash mismatch".into(),
        });
    }

    #[test]
    fn ack_roundtrips() {
        round_trip(FileMsg::Ack {
            id: 13,
            offset: 2 * 1024 * 1024,
        });
    }

    #[test]
    fn rejects_empty_and_unknown_variant() {
        assert!(FileMsg::from_bytes(&[]).is_none());
        assert!(FileMsg::from_bytes(&[0xFF]).is_none());
    }

    #[test]
    fn rejects_truncated_chunk() {
        let mut bytes = vec![4];
        bytes.extend_from_slice(&1u64.to_be_bytes());
        bytes.extend_from_slice(&2u64.to_be_bytes());
        bytes.extend_from_slice(&5u32.to_be_bytes());
        bytes.extend_from_slice(&[1, 2, 3]);
        assert!(FileMsg::from_bytes(&bytes).is_none());
    }
}
