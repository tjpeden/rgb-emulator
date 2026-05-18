/// DMG Audio Processing Unit — frame sequencer stub.
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
    // CH1–CH4 are stubbed; will be populated in future phases.
    _ch1: (),
    _ch2: (),
    _ch3: (),
    _ch4: (),
}

impl APU {
    pub fn new() -> Self {
        Self {
            frame_seq_step: 0,
            last_div_bit: false,
            _ch1: (),
            _ch2: (),
            _ch3: (),
            _ch4: (),
        }
    }

    /// Advance the APU by one instruction step.
    ///
    /// `div_counter` is the full 16-bit internal timer counter. The frame
    /// sequencer is clocked on the falling edge of bit 12.
    pub fn step(&mut self, div_counter: u16) {
        let current_div_bit = (div_counter >> 12) & 1 != 0;

        // Falling edge: bit was 1, now 0
        if self.last_div_bit && !current_div_bit {
            self.tick_frame_sequencer();
        }

        self.last_div_bit = current_div_bit;
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

    /// Clock length counters (256 Hz). Stub — no-op until channels are implemented.
    fn clock_length(&mut self) {}

    /// Clock volume envelopes (64 Hz). Stub — no-op until channels are implemented.
    fn clock_envelope(&mut self) {}

    /// Clock CH1 frequency sweep (128 Hz). Stub — no-op until CH1 is implemented.
    fn clock_sweep(&mut self) {}

    /// Read an APU register. Returns `0xFF` (open bus) for all addresses.
    pub fn read(&self, _addr: u16) -> u8 {
        0xFF
    }

    /// Write an APU register. Ignored until channels are implemented.
    pub fn write(&mut self, _addr: u16, _value: u8) {}

    /// Mix and return a stereo audio sample. Returns silence until channels
    /// are implemented.
    pub fn mix_samples(&self) -> (f32, f32) {
        (0.0, 0.0)
    }
}

impl Default for APU {
    fn default() -> Self {
        Self::new()
    }
}
