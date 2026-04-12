mod inline_parser;
mod opcodes;
#[cfg(test)]
mod tests;

pub use inline_parser::parse_inline;
pub use opcodes::{AddressingMode, Instruction, Opcode};

use std::collections::HashMap;

/// Assemble a list of instructions into bytes, resolving labels.
pub fn assemble(instructions: &[Instruction], base_address: u16) -> AssembleResult {
    let mut assembler = Assembler::new(base_address);
    assembler.assemble(instructions)
}

/// Assemble a single instruction into bytes (no label resolution).
pub fn assemble_instruction(opcode: Opcode, mode: &AddressingMode) -> Option<Vec<u8>> {
    let op = opcodes::encode(opcode, mode)?;
    let operand = mode.operand_bytes();
    let mut bytes = vec![op];
    bytes.extend_from_slice(&operand);
    Some(bytes)
}

pub struct AssembleResult {
    pub bytes: Vec<u8>,
    pub labels: HashMap<String, u16>,
}

struct Assembler {
    base_address: u16,
    output: Vec<u8>,
    labels: HashMap<String, u16>,
    fixups: Vec<Fixup>,
}

struct Fixup {
    offset: usize,
    label: String,
    kind: FixupKind,
}

enum FixupKind {
    Absolute, // 2-byte absolute address
    Relative, // 1-byte signed offset (branch)
    Lo,       // low byte of address
    Hi,       // high byte of address
}

impl Assembler {
    fn new(base_address: u16) -> Self {
        Self {
            base_address,
            output: Vec::new(),
            labels: HashMap::new(),
            fixups: Vec::new(),
        }
    }

    fn current_address(&self) -> u16 {
        self.base_address.wrapping_add(self.output.len() as u16)
    }

    fn assemble(&mut self, instructions: &[Instruction]) -> AssembleResult {
        // First pass: emit bytes, collect labels, record fixups
        for inst in instructions {
            self.emit_instruction(inst);
        }

        // Second pass: resolve fixups
        self.resolve_fixups();

        AssembleResult {
            bytes: self.output.clone(),
            labels: self.labels.clone(),
        }
    }

    fn emit_instruction(&mut self, inst: &Instruction) {
        match &inst.mode {
            AddressingMode::Label(name) => {
                // This should be a label definition, not an instruction
                self.labels.insert(name.clone(), self.current_address());
            }
            AddressingMode::LabelRelative(name) => {
                // Branch to label — emit opcode + placeholder
                if let Some(op) = opcodes::encode(inst.opcode, &AddressingMode::Relative(0)) {
                    self.output.push(op);
                    self.fixups.push(Fixup {
                        offset: self.output.len(),
                        label: name.clone(),
                        kind: FixupKind::Relative,
                    });
                    self.output.push(0); // placeholder
                }
            }
            AddressingMode::SymbolLo(name) => {
                if let Some(op) = opcodes::encode(inst.opcode, &AddressingMode::Immediate(0)) {
                    self.output.push(op);
                    self.fixups.push(Fixup {
                        offset: self.output.len(),
                        label: name.clone(),
                        kind: FixupKind::Lo,
                    });
                    self.output.push(0);
                }
            }
            AddressingMode::SymbolHi(name) => {
                if let Some(op) = opcodes::encode(inst.opcode, &AddressingMode::Immediate(0)) {
                    self.output.push(op);
                    self.fixups.push(Fixup {
                        offset: self.output.len(),
                        label: name.clone(),
                        kind: FixupKind::Hi,
                    });
                    self.output.push(0);
                }
            }
            mode => {
                if let Some(op) = opcodes::encode(inst.opcode, mode) {
                    self.output.push(op);
                    self.output.extend_from_slice(&mode.operand_bytes());
                } else if let Some(abs_addr) = mode.as_absolute_address() {
                    // Try encoding as absolute if we had a symbolic form
                    self.fixups.push(Fixup {
                        offset: self.output.len(),
                        label: format!("__abs_{abs_addr:04X}"),
                        kind: FixupKind::Absolute,
                    });
                }
            }
        }
    }

    fn resolve_fixups(&mut self) {
        for fixup in &self.fixups {
            if let Some(&addr) = self.labels.get(&fixup.label) {
                match fixup.kind {
                    FixupKind::Absolute => {
                        let bytes = addr.to_le_bytes();
                        self.output[fixup.offset] = bytes[0];
                        self.output[fixup.offset + 1] = bytes[1];
                    }
                    FixupKind::Relative => {
                        #[allow(clippy::cast_possible_wrap)]
                        let from = i32::from(self.base_address) + fixup.offset as i32 + 1;
                        let to = i32::from(addr);
                        let offset = to - from;
                        assert!(
                            (-128..=127).contains(&offset),
                            "branch offset {offset} out of range (-128..127) for label '{}'",
                            fixup.label
                        );
                        self.output[fixup.offset] = (offset as i8).cast_unsigned();
                    }
                    FixupKind::Lo => {
                        self.output[fixup.offset] = addr as u8;
                    }
                    FixupKind::Hi => {
                        self.output[fixup.offset] = (addr >> 8) as u8;
                    }
                }
            } else {
                panic!("unresolved label: '{}'", fixup.label);
            }
        }
    }
}
