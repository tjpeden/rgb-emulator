mod ch1;
mod ch2;
mod ch3;
mod ch4;

use ch1::CH1;
use ch2::CH2;
use ch3::CH3;
use ch4::CH4;

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
    /// CH4: Noise via LFSR.
    ch4: CH4,
    /// NR50: master volume register (0xFF24).
    nr50: u8,
    /// NR51: sound panning register (0xFF25).
    nr51: u8,
    /// NR52: sound enable register (0xFF26), bit 7 = master enable.
    nr52: u8,
    /// Accumulated left sample sum for downsampling.
    sample_accum_l: f32,
    /// Accumulated right sample sum for downsampling.
    sample_accum_r: f32,
    /// Number of T-cycles accumulated toward the next output sample.
    sample_accum_cycles: u32,
    /// Output ring buffer at 44100 Hz stereo.
    output_buffer: std::collections::VecDeque<(f32, f32)>,
}

impl APU {
    pub fn new() -> Self {
        Self {
            frame_seq_step: 0,
            last_div_bit: false,
            ch1: CH1::new(),
            ch2: CH2::new(),
            ch3: CH3::new(),
            ch4: CH4::new(),
            nr50: 0x77, // default: max volume on both sides
            nr51: 0xF3, // default panning from DMG post-boot
            nr52: 0xF1, // default: master on, CH1/CH2/CH4 active
            sample_accum_l: 0.0,
            sample_accum_r: 0.0,
            sample_accum_cycles: 0,
            output_buffer: std::collections::VecDeque::new(),
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
        self.ch4.step(cycles);

        // Accumulate mixed samples for downsampling to 44100 Hz.
        // DMG runs at 4194304 Hz; we need 1 output sample per ~95.1 T-cycles.
        // We use integer accumulation: every time accumulated cycles >= threshold,
        // push an averaged sample.
        const OUTPUT_RATE: u32 = 44100;
        const DMG_RATE: u32 = 4_194_304;

        let (l, r) = self.mix_samples();
        self.sample_accum_l += l * cycles as f32;
        self.sample_accum_r += r * cycles as f32;
        self.sample_accum_cycles += cycles;

        // Push one output sample for every DMG_RATE/OUTPUT_RATE T-cycles.
        while self.sample_accum_cycles * OUTPUT_RATE >= DMG_RATE {
            let out_l = self.sample_accum_l / self.sample_accum_cycles as f32;
            let out_r = self.sample_accum_r / self.sample_accum_cycles as f32;
            self.output_buffer.push_back((out_l, out_r));
            self.sample_accum_l = 0.0;
            self.sample_accum_r = 0.0;
            self.sample_accum_cycles = 0;
        }
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
        self.ch4.clock_length();
    }

    /// Clock volume envelopes (64 Hz).
    fn clock_envelope(&mut self) {
        self.ch1.clock_envelope();
        self.ch2.clock_envelope();
        self.ch4.clock_envelope();
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
            0xFF20..=0xFF23 => self.ch4.read(addr),
            0xFF24 => self.nr50,
            0xFF25 => self.nr51,
            0xFF26 => {
                // Bit 7 = master enable; bits 3-0 = channel active flags (read-only).
                let active = (self.ch4.active() as u8) << 3
                    | (self.ch3.active() as u8) << 2
                    | (self.ch2.active() as u8) << 1
                    | (self.ch1.active() as u8);
                (self.nr52 & 0x80) | 0x70 | active
            }
            _ => 0xFF,
        }
    }

    /// Write an APU register.
    pub fn write(&mut self, addr: u16, value: u8) {
        match addr {
            0xFF10..=0xFF14 => self.ch1.write(addr, value),
            0xFF16..=0xFF19 => self.ch2.write(addr, value),
            0xFF1A..=0xFF1E | 0xFF30..=0xFF3F => self.ch3.write(addr, value),
            0xFF20..=0xFF23 => self.ch4.write(addr, value),
            0xFF24 => self.nr50 = value,
            0xFF25 => self.nr51 = value,
            0xFF26 => self.nr52 = value & 0x80,
            _ => {}
        }
    }

    /// Mix and return a stereo audio sample applying NR51 panning and NR50 volume.
    pub fn mix_samples(&self) -> (f32, f32) {
        let ch1 = self.ch1.sample();
        let ch2 = self.ch2.sample();
        let ch3 = self.ch3.sample();
        let ch4 = self.ch4.sample();

        let left = (if self.nr51 & 0x10 != 0 { ch1 } else { 0.0 })
            + (if self.nr51 & 0x20 != 0 { ch2 } else { 0.0 })
            + (if self.nr51 & 0x40 != 0 { ch3 } else { 0.0 })
            + (if self.nr51 & 0x80 != 0 { ch4 } else { 0.0 });

        let right = (if self.nr51 & 0x01 != 0 { ch1 } else { 0.0 })
            + (if self.nr51 & 0x02 != 0 { ch2 } else { 0.0 })
            + (if self.nr51 & 0x04 != 0 { ch3 } else { 0.0 })
            + (if self.nr51 & 0x08 != 0 { ch4 } else { 0.0 });

        let left_vol = (((self.nr50 >> 4) & 0x07) as f32 + 1.0) / 8.0;
        let right_vol = ((self.nr50 & 0x07) as f32 + 1.0) / 8.0;

        // Divide by 4 to normalize across 4 channels.
        (left * left_vol / 4.0, right * right_vol / 4.0)
    }

    /// Drain and return all accumulated 44100 Hz output samples.
    pub fn drain_samples(&mut self) -> Vec<(f32, f32)> {
        self.output_buffer.drain(..).collect()
    }
}

impl Default for APU {
    fn default() -> Self {
        Self::new()
    }
}
