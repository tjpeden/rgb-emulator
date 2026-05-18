//! DMG APU Channel 4 — Noise via LFSR.
//!
//! Registers:
//! - NR41 (`0xFF20`): length data (bits 5-0)
//! - NR42 (`0xFF21`): initial volume (bits 7-4), envelope direction (bit 3), period (bits 2-0)
//! - NR43 (`0xFF22`): clock shift (bits 7-4), LFSR width (bit 3), divider ratio (bits 2-0)
//! - NR44 (`0xFF23`): trigger (bit 7), length enable (bit 6)

pub struct CH4 {
    // --- NR41: length ---
    length_counter: u8,

    // --- NR42: volume envelope ---
    initial_volume: u8,
    env_add: bool,
    env_period: u8,

    // --- NR43: polynomial counter ---
    clock_shift: u8,  // bits 7-4
    short_mode: bool, // bit 3 — 7-bit LFSR when true
    divider: u8,      // bits 2-0

    // --- NR44: control ---
    length_enable: bool,

    // --- Internal state ---
    enabled: bool,
    dac_enabled: bool,

    /// 15-bit LFSR (only bits 14-0 used).
    lfsr: u16,

    /// Frequency timer (counts down in T-cycles).
    freq_timer: u32,

    // Envelope runtime state
    env_volume: u8,
    env_timer: u8,
    env_running: bool,
}

impl CH4 {
    pub fn new() -> Self {
        Self {
            length_counter: 0,
            initial_volume: 0,
            env_add: false,
            env_period: 0,
            clock_shift: 0,
            short_mode: false,
            divider: 0,
            length_enable: false,
            enabled: false,
            dac_enabled: false,
            lfsr: 0x7FFF,
            freq_timer: 8,
            env_volume: 0,
            env_timer: 0,
            env_running: false,
        }
    }

    /// Compute the timer period from current NR43 settings.
    fn timer_period(&self) -> u32 {
        let base: u32 = if self.divider == 0 { 8 } else { u32::from(self.divider) * 16 };
        base << self.clock_shift
    }

    /// Read an NR4x register.
    pub fn read(&self, addr: u16) -> u8 {
        match addr {
            0xFF20 => 0xFF, // write-only
            0xFF21 => {
                ((self.initial_volume & 0x0F) << 4)
                    | if self.env_add { 0x08 } else { 0 }
                    | (self.env_period & 0x07)
            }
            0xFF22 => {
                ((self.clock_shift & 0x0F) << 4)
                    | if self.short_mode { 0x08 } else { 0 }
                    | (self.divider & 0x07)
            }
            0xFF23 => 0xBF | if self.length_enable { 0x40 } else { 0 },
            _ => 0xFF,
        }
    }

    /// Write an NR4x register.
    pub fn write(&mut self, addr: u16, value: u8) {
        match addr {
            0xFF20 => {
                self.length_counter = 64 - (value & 0x3F);
            }
            0xFF21 => {
                self.initial_volume = (value >> 4) & 0x0F;
                self.env_add = (value & 0x08) != 0;
                self.env_period = value & 0x07;
                self.dac_enabled = (value & 0xF8) != 0;
                if !self.dac_enabled {
                    self.enabled = false;
                }
            }
            0xFF22 => {
                self.clock_shift = (value >> 4) & 0x0F;
                self.short_mode = (value & 0x08) != 0;
                self.divider = value & 0x07;
            }
            0xFF23 => {
                self.length_enable = (value & 0x40) != 0;
                if (value & 0x80) != 0 {
                    self.trigger();
                }
            }
            _ => {}
        }
    }

    fn trigger(&mut self) {
        self.enabled = self.dac_enabled;

        if self.length_counter == 0 {
            self.length_counter = 64;
        }

        // Reload envelope
        self.env_volume = self.initial_volume;
        self.env_timer = self.env_period;
        self.env_running = true;

        // Reset LFSR
        self.lfsr = 0x7FFF;

        // Reload frequency timer
        self.freq_timer = self.timer_period();
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

    /// Advance the LFSR timer by `cycles` T-cycles.
    pub fn step(&mut self, cycles: u32) {
        if !self.enabled {
            return;
        }

        let mut remaining = cycles;
        while remaining > 0 {
            if self.freq_timer <= remaining {
                remaining -= self.freq_timer;
                self.clock_lfsr();
                self.freq_timer = self.timer_period();
                if self.freq_timer == 0 {
                    break;
                }
            } else {
                self.freq_timer -= remaining;
                remaining = 0;
            }
        }
    }

    fn clock_lfsr(&mut self) {
        let xor_bit = (self.lfsr & 1) ^ ((self.lfsr >> 1) & 1);
        self.lfsr >>= 1;
        self.lfsr |= xor_bit << 14;
        if self.short_mode {
            self.lfsr = (self.lfsr & !(1 << 6)) | (xor_bit << 6);
        }
    }

    /// Return the current sample as f32 in [-1.0, 1.0]. Returns 0.0 if disabled.
    pub fn sample(&self) -> f32 {
        if !self.enabled || !self.dac_enabled {
            return 0.0;
        }
        // Output bit is inverted LSB
        let output_bit: u8 = if (self.lfsr & 1) == 0 { 1 } else { 0 };
        let raw = output_bit * self.env_volume; // 0-15
        (f32::from(raw) / 7.5) - 1.0
    }
}

impl Default for CH4 {
    fn default() -> Self {
        Self::new()
    }
}
