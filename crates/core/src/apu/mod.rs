mod ch1;
mod ch2;
mod ch3;

use ch1::CH1;
use ch2::CH2;
use ch3::CH3;

/// DMG Audio Processing Unit — frame sequencer + CH1 + CH2 + CH3.
///
/// The APU frame sequencer is clocked at 512 Hz by detecting the falling edge
/// of bit 12 of the internal 16-bit DIV counter (equivalent to bit 4 of the
/// DIV register, the upper byte). It steps through 8 positions (0–7):
///
/// | Step | Length (256 Hz) | Envelope (64 Hz) | Sweep (128 Hz) |
/// |------|-----------------|------------------|----------------|
/// |  0   | clock           |                  |                |
/// |  1   |                 |                  |                |
/// |  2   | clock           |                  | clock          |
/// |  3   |                 |                  |                |
/// |  4   | clock           |                  |                |
/// |  5   |                 |                  |                |
/// |  6   | clock           |                  | clock          |
/// |  7   |                 | clock            |                |
pub struct APU {
    /// Current frame sequencer step (0–7).
    frame_seq_step: u8,
    /// Last observed value of bit 12 of the DIV counter, used to detect
    /// the falling edge that clocks the frame sequencer.
    last_div_bit: bool,
    /// CH1: Pulse wave with frequency sweep.
    ch1: CH1,
    /// CH2: Pulse wave (no sweep).
    ch2: CH2,
    /// CH3: Arbitrary waveform.
    ch3: CH3,
    // CH4 is stubbed; will be populated in a future phase.
    _ch4: (),
}

impl APU {
    pub fn new() -> Self {
        Self {
            frame_seq_step: 0,
            last_div_bit: false,
            ch1: CH1::new(),
            ch2: CH2::new(),
            ch3: CH3::new(),
            _ch4: (),
        }
    }

    /// Advance the APU by `cycles` T-cycles.
    ///
    /// `div_counter` is the full 16-bit internal timer counter. The frame
    /// sequencer is clocked on the falling edge of bit 12.
    pub fn step(&mut self, div_counter: u16, cycles: u32) {
        let current_div_bit = (div_counter >> 12) & 1 != 0;

        // Falling edge: bit was 1, now 0
        if self.last_div_bit && !current_div_bit {
            self.tick_frame_sequencer();
        }

        self.last_div_bit = current_div_bit;

        self.ch1.step(cycles);
        self.ch2.step(cycles);
        self.ch3.step(cycles);
    }

    fn tick_frame_sequencer(&mut self) {
        match self.frame_seq_step {
            0 | 4 => {
                self.clock_length();
            }
            2 | 6 => {
                self.clock_length();
                self.clock_sweep();
            }
            7 => {
                self.clock_envelope();
            }
            _ => {}
        }

        self.frame_seq_step = (self.frame_seq_step + 1) & 7;
    }

    /// Clock length counters (256 Hz).
    fn clock_length(&mut self) {
        self.ch1.clock_length();
        self.ch2.clock_length();
        self.ch3.clock_length();
    }

    /// Clock volume envelopes (64 Hz).
    fn clock_envelope(&mut self) {
        self.ch1.clock_envelope();
        self.ch2.clock_envelope();
    }

    /// Clock CH1 frequency sweep (128 Hz).
    fn clock_sweep(&mut self) {
        self.ch1.clock_sweep();
    }

    /// Read an APU register.
    pub fn read(&self, addr: u16) -> u8 {
        match addr {
            0xFF10..=0xFF14 => self.ch1.read(addr),
            0xFF16..=0xFF19 => self.ch2.read(addr),
            0xFF1A..=0xFF1E | 0xFF30..=0xFF3F => self.ch3.read(addr),
            _ => 0xFF,
        }
    }

    /// Write an APU register.
    pub fn write(&mut self, addr: u16, value: u8) {
        match addr {
            0xFF10..=0xFF14 => self.ch1.write(addr, value),
            0xFF16..=0xFF19 => self.ch2.write(addr, value),
            0xFF1A..=0xFF1E | 0xFF30..=0xFF3F => self.ch3.write(addr, value),
            _ => {}
        }
    }

    /// Mix and return a stereo audio sample.
    pub fn mix_samples(&self) -> (f32, f32) {
        let ch1 = self.ch1.sample();
        let ch2 = self.ch2.sample();
        let ch3 = self.ch3.sample();
        let left = ch1 + ch2 + ch3;
        let right = ch1 + ch2 + ch3;
        (left, right)
    }
}

impl Default for APU {
    fn default() -> Self {
        Self::new()
    }
}
