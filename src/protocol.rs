#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Mode01 {
    pid: u8,
    data_len: usize,
}

impl Mode01 {
    pub(crate) const fn new(pid: u8, data_len: usize) -> Self {
        Self { pid, data_len }
    }

    pub(crate) const fn bytes(self) -> [u8; 2] {
        [0x01, self.pid]
    }

    pub(crate) const fn pid(self) -> u8 {
        self.pid
    }

    pub(crate) const fn data_len(self) -> usize {
        self.data_len
    }

    pub(crate) fn data<'a>(self, response: &'a [u8], semantic: &str) -> Result<&'a [u8], String> {
        let expected = self.data_len + 2;
        if response.len() != expected || response[..2] != [0x41, self.pid] {
            return Err(format!(
                "invalid OBD-II {semantic} response: {}",
                crate::hex(response)
            ));
        }
        Ok(&response[2..])
    }
}
