//! A small x64 assembler for generated effects: bytes, rel32 conditional branches bound later, and exits that all
//! land at the end of the code, where the caller's gateway continues (to the original, or past it).
#[derive(Default)]
pub struct Code {
    pub bytes: Vec<u8>,
    exits: Vec<usize>,
}

impl Code {
    pub fn emit(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }
    /// An instruction whose last operand is a 32-bit field offset.
    pub fn offset(&mut self, instruction: &[u8], offset: u32) {
        self.emit(instruction);
        self.emit(&offset.to_le_bytes());
    }
    /// A `0f cc rel32` branch, bound later by `bind` or `bind_at`; its displacement's place in the code.
    pub fn branch(&mut self, condition: u8) -> usize {
        self.emit(&[0x0f, condition]);
        let at = self.bytes.len();
        self.emit(&[0; 4]);
        at
    }
    /// A branch to the end of the code.
    pub fn exit(&mut self, condition: u8) {
        let at = self.branch(condition);
        self.exits.push(at);
    }
    /// An unconditional jump to the end of the code.
    pub fn done(&mut self) {
        self.emit(&[0xe9]);
        self.exits.push(self.bytes.len());
        self.emit(&[0; 4]);
    }
    /// A test, then an exit when it sets ZF (a null register).
    pub fn nonnull(&mut self, test: &[u8]) {
        self.emit(test);
        self.exit(0x84);
    }
    /// The branch at `at` lands here.
    pub fn bind(&mut self, at: usize) {
        self.bind_at(at, self.bytes.len());
    }
    /// The branch at `at` lands at `target`, before or after it.
    pub fn bind_at(&mut self, at: usize, target: usize) {
        let displacement = i32::try_from(target as isize - (at + 4) as isize).expect("bounded generated effect");
        self.bytes[at..at + 4].copy_from_slice(&displacement.to_le_bytes());
    }
    /// The code, every exit bound to its end.
    pub fn finish(mut self) -> Vec<u8> {
        for at in std::mem::take(&mut self.exits) {
            self.bind(at);
        }
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branches_bind_forward_and_back_and_exits_land_at_the_end() {
        let mut c = Code::default();
        let top = c.bytes.len();
        c.emit(&[0x90]);
        let forward = c.branch(0x84);
        c.exit(0x85);
        c.bind(forward);
        let back = c.branch(0x8e);
        c.bind_at(back, top);
        c.done();
        let code = c.finish();
        let rel = |at: usize| i32::from_le_bytes(code[at..at + 4].try_into().unwrap());
        assert_eq!(rel(3), 6, "forward lands after the exit");
        assert_eq!(rel(9), 24 - 13, "the exit lands at the end");
        assert_eq!(rel(15), -19, "back lands at the top");
        assert_eq!(rel(20), 0, "done lands at the end");
        assert_eq!(code.len(), 24);
    }
}
