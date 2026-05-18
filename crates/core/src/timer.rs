/// DMG timer hardware.
///
/// The timer is driven by a single internal 16-bit counter that increments
/// every T-cycle. DIV is the upper byte; any write to DIV resets the full
/// counter. TIMA is clocked by a specific bit of the counter determined by TAC.
pub struct Timer {
    /// Internal 16-bit counter. Increments every T-cycle.
    counter: u16,
    /// TIMA — Timer Counter (0xFF05)
    tima: u8,
    /// TMA — Timer Modulo (0xFF06)
    tma: u8,
    /// TAC — Timer Control (0xFF07)
    tac: u8,
    /// Overflow delay counter. When TIMA overflows, there is a 4 T-cycle
    /// window where TIMA = 0x00 before it reloads from TMA and fires IRQ.
    overflow_delay: u8,
}

impl Timer {
    pub fn new() -> Self {
        Self {
            counter: 0,
            tima: 0,
            tma: 0,
            tac: 0,
            overflow_delay: 0,
        }
    }

    /// Return the counter bit index selected by TAC bits 1–0.
    fn clock_bit(&self) -> u16 {
        match self.tac & 0x03 {
            0b00 => 9,
            0b01 => 3,
            0b10 => 5,
            0b11 => 7,
            _ => unreachable!(),
        }
    }

    /// Advance the timer by `t_cycles` T-cycles.
    ///
    /// Returns `true` if a timer interrupt should be requested (set IF bit 2).
    pub fn step(&mut self, t_cycles: u32) -> bool {
        let mut irq = false;

        for _ in 0..t_cycles {
            // Handle overflow delay: TIMA stays 0x00 for 4 T-cycles after
            // overflow, then reloads from TMA and fires the interrupt.
            if self.overflow_delay > 0 {
                self.overflow_delay -= 1;
                if self.overflow_delay == 0 {
                    self.tima = self.tma;
                    irq = true;
                }
            }

            let prev_counter = self.counter;
            self.counter = self.counter.wrapping_add(1);

            // Clock TIMA on falling edge of the selected bit when timer enabled.
            let bit = self.clock_bit();
            let timer_enabled = (self.tac & 0x04) != 0;
            let prev_bit = (prev_counter >> bit) & 1;
            let new_bit = (self.counter >> bit) & 1;

            if timer_enabled && prev_bit == 1 && new_bit == 0 {
                let (new_tima, overflow) = self.tima.overflowing_add(1);
                self.tima = new_tima;
                if overflow {
                    self.tima = 0x00;
                    self.overflow_delay = 4;
                }
            }
        }

        irq
    }

    /// Read a timer register.
    pub fn read(&self, addr: u16) -> u8 {
        match addr {
            0xFF04 => (self.counter >> 8) as u8, // DIV = upper byte
            0xFF05 => self.tima,
            0xFF06 => self.tma,
            0xFF07 => self.tac,
            _ => unreachable!("Timer::read called with invalid addr {:#06X}", addr),
        }
    }

    /// Return the raw 16-bit internal counter.
    ///
    /// Used by the APU frame sequencer to detect falling edges on bit 12.
    pub fn div_counter(&self) -> u16 {
        self.counter
    }

    /// Write a timer register.
    pub fn write(&mut self, addr: u16, value: u8) {
        match addr {
            0xFF04 => self.counter = 0, // Any write resets the full counter
            0xFF05 => self.tima = value,
            0xFF06 => self.tma = value,
            0xFF07 => self.tac = value & 0x07,
            _ => unreachable!("Timer::write called with invalid addr {:#06X}", addr),
        }
    }
}

impl Default for Timer {
    fn default() -> Self {
        Self::new()
    }
}
