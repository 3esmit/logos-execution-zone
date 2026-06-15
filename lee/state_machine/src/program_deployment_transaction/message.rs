use borsh::{BorshDeserialize, BorshSerialize};

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Message {
    pub(crate) bytecode: Vec<u8>,
}

impl Message {
    #[must_use]
    pub const fn new(bytecode: Vec<u8>) -> Self {
        Self { bytecode }
    }

    #[must_use]
    pub fn into_bytecode(self) -> Vec<u8> {
        self.bytecode
    }
}

#[cfg(test)]
mod tests {
    use super::Message;

    #[test]
    fn bytecode_roundtrip() {
        // `Message::new(b).into_bytecode()` must return exactly `b`. Catches
        // mutations of `into_bytecode` returning `vec![]`, `vec![0]`, or `vec![1]`.
        let bytecode = vec![0x7F_u8, 0x45, 0x4C, 0x46]; // ELF magic
        assert_eq!(Message::new(bytecode.clone()).into_bytecode(), bytecode);
        assert!(Message::new(vec![]).into_bytecode().is_empty());
        assert_eq!(Message::new(vec![0xAB]).into_bytecode(), vec![0xAB_u8]);
    }
}
