//! DMG APU Channel 3 — Arbitrary waveform.
//!
//! Registers:
//! - NR30 (`0xFF1A`): DAC enable (bit 7)
//! - NR31 (`0xFF1B`): length data (bits 7-0); length counter = 256 - val
//! - NR32 (`0xFF1C`): output level (bits 6-5): 0=mute, 1=100%, 2=50%, 3=25%
//! - NR33 (`0xFF1D`): frequency low byte (write-only)
//! - NR34 (`0xFF1E`): trigger (bit 7), length enable (bit 6), freq high (bits 2-0)
//!
//! Wave RAM: `0xFF30–0xFF3F` — 16 bytes, 32 × 4-bit samples

pub struct CH3 {
    // --- NR30: DAC ---
    dac_enabled: bool,

    // --- NR31: length ---
    length_counter: u16, // 256 - NR31 value

    // --- NR32: volume ---
    output_level: u8, // 0-3

    // --- NR33/NR34: frequency ---
    freq: u16,
    length_enable: bool,

    // --- Internal state ---
    enabled: bool,

    // Frequency timer (counts down in T-cycles)
    freq_timer: u32,
    // Current sample position (0-31)
    sample_pos: u8,

    // Wave RAM (16 bytes)
    wave_ram: [u8; 16],
}

impl CH3 {
    pub fn new() -> Self {
        Self {
            dac_enabled: false,
            length_counter: 0,
            output_level: 0,
            freq: 0,
            length_enable: false,
            enabled: false,
            freq_timer: 4096,
            sample_pos: 0,
            wave_ram: [0u8; 16],
        }
    }

    /// Read an NR3x register or wave RAM.
    pub fn read(&self, addr: u16) -> u8 {
        match addr {
            0xFF1A => if self.dac_enabled { 0xFF } else { 0x7F },
            0xFF1B => 0xFF, // write-only
            0xFF1C => 0x9F | ((self.output_level & 0x03) << 5),
            0xFF1D => 0xFF, // write-only
            0xFF1E => 0xBF | if self.length_enable { 0x40 } else { 0 },
            0xFF30..=0xFF3F => {
                // On DMG, reading wave RAM while channel is active returns the
                // last accessed byte (obscure). Implement as open bus for now.
                if self.enabled {
                    0xFF
                } else {
                    self.wave_ram[(addr - 0xFF30) as usize]
                }
            }
            _ => 0xFF,
        }
    }

    /// Write an NR3x register or wave RAM.
    pub fn write(&mut self, addr: u16, value: u8) {
        match addr {
            0xFF1A => {
                self.dac_enabled = (value & 0x80) != 0;
                if !self.dac_enabled {
                    self.enabled = false;
                }
            }
            0xFF1B => {
                self.length_counter = 256 - u16::from(value);
            }
            0xFF1C => {
                self.output_level = (value >> 5) & 0x03;
            }
            0xFF1D => {
                self.freq = (self.freq & 0x0700) | u16::from(value);
            }
            0xFF1E => {
                self.freq = (self.freq & 0x00FF) | (u16::from(value & 0x07) << 8);
                self.length_enable = (value & 0x40) != 0;
                if (value & 0x80) != 0 {
                    self.trigger();
                }
            }
            0xFF30..=0xFF3F => {
                self.wave_ram[(addr - 0xFF30) as usize] = value;
            }
            _ => {}
        }
    }

    fn trigger(&mut self) {
        self.enabled = self.dac_enabled;

        // Reload length counter
        if self.length_counter == 0 {
            self.length_counter = 256;
        }

        // Reset sample position
        self.sample_pos = 0;

        // Reload frequency timer: period = (2048 - freq) * 2
        self.freq_timer = (2048 - u32::from(self.freq)) * 2;
    }

    /// Clock length counter (frame sequencer steps 0, 2, 4, 6 — 256 Hz).
    pub fn clock_length(&mut self) {
        if self.length_enable && self.length_counter > 0 {
            self.length_counter -= 1;
            if self.length_counter == 0 {
                self.enabled = false;
            }
        }
    }

    /// Advance the frequency timer by `cycles` T-cycles and update sample position.
    pub fn step(&mut self, cycles: u32) {
        if !self.enabled {
            return;
        }

        let period = (2048 - u32::from(self.freq)) * 2;
        if period == 0 {
            return;
        }

        if self.freq_timer <= cycles {
            let remainder = cycles - self.freq_timer;
            self.sample_pos = (self.sample_pos + 1) & 31;
            self.freq_timer = period;
            if remainder > 0 {
                let extra_steps = remainder / period;
                self.sample_pos = (self.sample_pos + extra_steps as u8) & 31;
                self.freq_timer -= remainder % period;
            }
        } else {
            self.freq_timer -= cycles;
        }
    }

    /// Return the current sample as f32 in [-1.0, 1.0]. Returns 0.0 if disabled.
    pub fn sample(&self) -> f32 {
        if !self.enabled || !self.dac_enabled {
            return 0.0;
        }

        let byte = self.wave_ram[(self.sample_pos / 2) as usize];
        let nibble = if self.sample_pos & 1 == 0 {
            (byte >> 4) & 0x0F // high nibble for even position
        } else {
            byte & 0x0F // low nibble for odd position
        };

        let shifted = match self.output_level {
            0 => 0,          // mute
            1 => nibble,     // 100%
            2 => nibble >> 1, // 50%
            3 => nibble >> 2, // 25%
            _ => 0,
        };

        // Map 0-15 to [-1.0, 1.0]
        (f32::from(shifted) - 7.5) / 7.5
    }

    /// Returns true if the channel is currently active.
    pub fn active(&self) -> bool {
        self.enabled && self.dac_enabled
    }
}

impl Default for CH3 {
    fn default() -> Self {
        Self::new()
    }
}
