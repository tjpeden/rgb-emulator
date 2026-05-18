//! DMG APU Channel 2 — Pulse wave (no sweep).
//!
//! Registers:
//! - NR21 (`0xFF16`): duty cycle (bits 7-6), length data (bits 5-0)
//! - NR22 (`0xFF17`): initial volume (bits 7-4), envelope direction (bit 3), period (bits 2-0)
//! - NR23 (`0xFF18`): frequency low byte (write-only)
//! - NR24 (`0xFF19`): trigger (bit 7), length enable (bit 6), frequency high (bits 2-0)

/// Duty cycle waveform table. Each entry is an 8-bit pattern; bit 7 = step 0.
const DUTY_TABLE: [[u8; 8]; 4] = [
    [0, 0, 0, 0, 0, 0, 0, 1], // 12.5%
    [1, 0, 0, 0, 0, 0, 0, 1], // 25%
    [1, 0, 0, 0, 1, 1, 1, 1], // 50%
    [0, 1, 1, 1, 1, 1, 1, 0], // 75%
];

pub struct CH2 {
    // --- NR21: duty/length ---
    duty: u8,           // bits 7-6 (0-3)
    length_counter: u8, // 64 - (NR21 bits 5-0)

    // --- NR22: volume envelope ---
    initial_volume: u8, // bits 7-4
    env_add: bool,      // bit 3
    env_period: u8,     // bits 2-0

    // --- NR23/NR24: frequency ---
    freq: u16,          // 11-bit frequency value
    length_enable: bool, // NR24 bit 6

    // --- Internal state ---
    enabled: bool,
    dac_enabled: bool,

    // Frequency timer (counts down in T-cycles)
    freq_timer: u32,
    // Current waveform position (0-7)
    wave_pos: u8,

    // Envelope runtime state
    env_volume: u8,
    env_timer: u8,
    env_running: bool,
}

impl CH2 {
    pub fn new() -> Self {
        Self {
            duty: 2,
            length_counter: 0,
            initial_volume: 0,
            env_add: false,
            env_period: 0,
            freq: 0,
            length_enable: false,
            enabled: false,
            dac_enabled: false,
            freq_timer: 8192,
            wave_pos: 0,
            env_volume: 0,
            env_timer: 0,
            env_running: false,
        }
    }

    /// Read an NR2x register.
    pub fn read(&self, addr: u16) -> u8 {
        match addr {
            0xFF16 => 0x3F | ((self.duty & 0x03) << 6), // length bits read as 1
            0xFF17 => {
                ((self.initial_volume & 0x0F) << 4)
                    | if self.env_add { 0x08 } else { 0 }
                    | (self.env_period & 0x07)
            }
            0xFF18 => 0xFF, // write-only
            0xFF19 => 0xBF | if self.length_enable { 0x40 } else { 0 },
            _ => 0xFF,
        }
    }

    /// Write an NR2x register.
    pub fn write(&mut self, addr: u16, value: u8) {
        match addr {
            0xFF16 => {
                self.duty = (value >> 6) & 0x03;
                self.length_counter = 64 - (value & 0x3F);
            }
            0xFF17 => {
                self.initial_volume = (value >> 4) & 0x0F;
                self.env_add = (value & 0x08) != 0;
                self.env_period = value & 0x07;
                self.dac_enabled = (value & 0xF8) != 0;
                if !self.dac_enabled {
                    self.enabled = false;
                }
            }
            0xFF18 => {
                self.freq = (self.freq & 0x0700) | u16::from(value);
            }
            0xFF19 => {
                self.freq = (self.freq & 0x00FF) | (u16::from(value & 0x07) << 8);
                self.length_enable = (value & 0x40) != 0;
                if (value & 0x80) != 0 {
                    self.trigger();
                }
            }
            _ => {}
        }
    }

    /// Trigger (NR24 bit 7 write).
    fn trigger(&mut self) {
        self.enabled = self.dac_enabled;

        // Reload length counter
        if self.length_counter == 0 {
            self.length_counter = 64;
        }

        // Reload frequency timer
        self.freq_timer = (2048 - u32::from(self.freq)) * 4;

        // Reload volume envelope
        self.env_volume = self.initial_volume;
        self.env_timer = self.env_period;
        self.env_running = true;
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

    /// Clock volume envelope (frame sequencer step 7 — 64 Hz).
    pub fn clock_envelope(&mut self) {
        if self.env_period == 0 || !self.env_running {
            return;
        }
        if self.env_timer > 0 {
            self.env_timer -= 1;
        }
        if self.env_timer == 0 {
            self.env_timer = self.env_period;
            if self.env_add && self.env_volume < 15 {
                self.env_volume += 1;
            } else if !self.env_add && self.env_volume > 0 {
                self.env_volume -= 1;
            } else {
                self.env_running = false;
            }
        }
    }

    /// Advance the frequency timer by `cycles` T-cycles and update wave position.
    pub fn step(&mut self, cycles: u32) {
        if self.freq_timer <= cycles {
            let remainder = cycles - self.freq_timer;
            self.wave_pos = (self.wave_pos + 1) & 7;
            self.freq_timer = (2048 - u32::from(self.freq)) * 4;
            // Handle case where remainder is larger than period
            if self.freq_timer > 0 && remainder > 0 {
                let extra_steps = remainder / self.freq_timer;
                self.wave_pos = (self.wave_pos + extra_steps as u8) & 7;
                self.freq_timer -= remainder % self.freq_timer;
            }
        } else {
            self.freq_timer -= cycles;
        }
    }

    /// Return the current sample as f32 in [-1.0, 1.0]. Returns 0.0 if channel or DAC disabled.
    pub fn sample(&self) -> f32 {
        if !self.enabled || !self.dac_enabled {
            return 0.0;
        }
        let bit = DUTY_TABLE[self.duty as usize][self.wave_pos as usize];
        let raw = bit * self.env_volume; // 0-15
        // Map 0-15 to [-1.0, 1.0]: (raw / 7.5) - 1.0
        (f32::from(raw) / 7.5) - 1.0
    }

    /// Returns true if the channel is currently active.
    pub fn active(&self) -> bool {
        self.enabled && self.dac_enabled
    }
}

impl Default for CH2 {
    fn default() -> Self {
        Self::new()
    }
}
