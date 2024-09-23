use crate::bit_reader::{BitReader, BitReader2};
use crate::core::Core;
use crate::error::{ErrorContext, OozError, WithContext};
use crate::pointer::Pointer;

#[derive(Default)]
pub struct TansDecoder {
    pub lut: Vec<TansLutEnt>,
    pub dst: Pointer,
    pub dst_end: Pointer,
    pub ptr_f: Pointer,
    pub ptr_b: Pointer,
    pub bits_f: usize,
    pub bits_b: usize,
    pub bitpos_f: i32,
    pub bitpos_b: i32,
    pub state: [usize; 5],
}

impl ErrorContext for TansDecoder {}

impl TansDecoder {
    pub fn decode(&mut self, core: &mut Core) -> Result<(), OozError> {
        assert!(
            self.ptr_f <= self.ptr_b,
            "{:?} > {:?}",
            self.ptr_f,
            self.ptr_b
        );

        let mut step = 0;
        while self.dst < self.dst_end {
            if step < 5 {
                if step & 1 == 0 {
                    self.tans_forward_bits(core).at(self)?;
                }
                self.tans_forward_round(core, step).at(self)?;
            } else {
                if step & 1 == 1 {
                    self.tans_backward_bits(core).at(self)?;
                }
                self.tans_backward_round(core, step - 5).at(self)?;
            }
            step = (step + 1) % 10;
        }

        assert_eq!(
            self.ptr_b + (self.bitpos_f >> 3) + (self.bitpos_b >> 3),
            self.ptr_f
        );

        let states_or = self.state.iter().fold(0, |l, &r| l | r);
        assert_eq!(states_or & !0xFF, 0, "{:X}", states_or);

        core.set_bytes(self.dst_end, &self.state.map(|s| s as u8));
        Ok(())
    }

    fn tans_forward_bits(&mut self, core: &mut Core) -> Result<(), OozError> {
        self.bits_f |= core.get_le_bytes(self.ptr_f, 4).at(core)? << self.bitpos_f;
        self.ptr_f += (31 - self.bitpos_f) >> 3;
        self.bitpos_f |= 24;
        Ok(())
    }

    fn tans_forward_round(&mut self, core: &mut Core, i: usize) -> Result<(), OozError> {
        let e = &self.lut[self.state[i]];
        core.set(self.dst, e.symbol);
        self.dst += 1;
        self.bitpos_f -= e.bits_x as i32;
        self.state[i] = (self.bits_f & e.x as usize) + e.w as usize;
        self.bits_f >>= e.bits_x;
        Ok(())
    }

    fn tans_backward_bits(&mut self, core: &mut Core) -> Result<(), OozError> {
        self.bits_b |= core.get_be_bytes((self.ptr_b - 4)?, 4).at(core)? << self.bitpos_b;
        self.ptr_b -= (31 - self.bitpos_b) >> 3;
        self.bitpos_b |= 24;
        Ok(())
    }

    fn tans_backward_round(&mut self, core: &mut Core, i: usize) -> Result<(), OozError> {
        let e = &self.lut[self.state[i]];
        core.set(self.dst, e.symbol);
        self.dst += 1;
        self.bitpos_b -= e.bits_x as i32;
        self.state[i] = (self.bits_b & e.x as usize) + e.w as usize;
        self.bits_b >>= e.bits_x;
        Ok(())
    }

    pub fn init_lut(&self, tans_data: &TansData, l_bits: i32) -> Vec<TansLutEnt> {
        let mut pointers = [0usize; 4];

        let l = 1 << l_bits;
        let len = l as usize;
        let a_used = tans_data.a_used as usize;

        let slots_left_to_alloc = len - a_used;

        let sa = slots_left_to_alloc >> 2;
        let mut sb = sa;
        if (slots_left_to_alloc & 3) > 0 {
            sb += 1;
        }
        pointers[1] = sb;
        sb += sa;
        if (slots_left_to_alloc & 3) > 1 {
            sb += 1;
        }
        pointers[2] = sb;
        sb += sa;
        if (slots_left_to_alloc & 3) > 2 {
            sb += 1;
        }
        pointers[3] = sb;

        let mut lut = Vec::with_capacity(len);
        lut.resize_with(len, TansLutEnt::default);
        // Set up the single entries with weight=1
        {
            for i in 0..a_used {
                let le = &mut lut[slots_left_to_alloc + i];
                le.w = 0;
                le.bits_x = l_bits as u8;
                le.x = (1 << l_bits) - 1;
                le.symbol = tans_data.a[i];
            }
        }

        // Set up the entries with weight >= 2
        let mut weights_sum = 0;
        for i in 0..(tans_data.b_used as usize) {
            let weight = (tans_data.b[i] & 0xffff) as i32;
            let symbol = (tans_data.b[i] >> 16) as i32;
            if weight > 4 {
                let sym_bits = weight.ilog2() as i32;
                let mut z = l_bits - sym_bits;
                let mut le = TansLutEnt {
                    symbol: symbol as u8,
                    bits_x: z as u8,
                    x: (1 << z) - 1,
                    w: ((l - 1) & (weight << z)) as u16,
                };

                let mut what_to_add = 1 << z;
                let mut x = (1 << (sym_bits + 1)) - weight;

                for j in 0..4 {
                    let mut dst = pointers[j as usize];

                    let y = (weight + ((weights_sum - j - 1) & 3)) >> 2;
                    if x >= y {
                        for _ in 0..y {
                            lut[dst] = le;
                            dst += 1;
                            le.w += what_to_add;
                        }
                        x -= y;
                    } else {
                        for _ in 0..x {
                            lut[dst] = le;
                            dst += 1;
                            le.w += what_to_add;
                        }
                        z -= 1;

                        what_to_add >>= 1;
                        le.bits_x = z as u8;
                        le.w = 0;
                        le.x >>= 1;
                        for _ in 0..y - x {
                            lut[dst] = le;
                            dst += 1;
                            le.w += what_to_add;
                        }
                        x = weight;
                    }
                    pointers[j as usize] = dst;
                }
            } else {
                assert!(weight > 0);
                let mut bits: u32 = ((1 << weight) - 1) << (weights_sum & 3);
                bits |= bits >> 4;
                let mut ww = weight;
                for _ in 0..weight {
                    let idx = bits.trailing_zeros() as usize;
                    bits &= bits - 1;
                    let dst = pointers[idx];
                    pointers[idx] += 1;
                    lut[dst].symbol = symbol as u8;
                    let weight_bits = ww.ilog2() as i32;
                    lut[dst].bits_x = (l_bits - weight_bits) as u8;
                    lut[dst].x = (1 << (l_bits - weight_bits)) - 1;
                    lut[dst].w = ((l - 1) & (ww << (l_bits - weight_bits))) as u16;
                    ww += 1;
                }
            }
            weights_sum += weight;
        }
        lut
    }

    /// Tans_DecodeTable
    pub fn decode_table(
        &mut self,
        core: &mut Core,
        bits: &mut BitReader,
        l_bits: i32,
    ) -> Result<TansData, OozError> {
        let mut tans_data = TansData {
            a_used: 0,
            b_used: 0,
            a: [0; 256],
            b: [0; 256],
        };
        bits.refill(core).at(self)?;
        if bits.read_bit_no_refill() {
            let q = bits.read_bits_no_refill(3);
            let num_symbols = bits.read_bits_no_refill(8) + 1;
            assert!(num_symbols >= 2);
            let fluff = bits.read_fluff(num_symbols);
            let total_rice_values = num_symbols as usize + fluff;
            let mut rice = [0; 512 + 16];

            // another bit reader...
            let mut br2 = BitReader2 {
                p: (bits.p - ((24 - bits.bitpos + 7) >> 3) as u32)?,
                p_end: bits.p_end,
                bitpos: ((bits.bitpos - 24) & 7) as u32,
            };

            core.decode_golomb_rice_lengths(&mut rice[..total_rice_values], &mut br2)
                .at(&mut tans_data)?;

            // Switch back to other bitreader impl
            bits.bitpos = 24;
            bits.p = br2.p;
            bits.bits = 0;
            bits.refill(core).at(self)?;
            bits.bits <<= br2.bitpos;
            bits.bitpos += br2.bitpos as i32;

            let range = core
                .convert_to_ranges(num_symbols, fluff, &rice, bits)
                .at(&mut tans_data)?;

            bits.refill(core).at(self)?;

            let l = 1 << l_bits;
            let mut cur_rice_ptr: &[u8] = &rice;
            let mut average = 6;
            let mut somesum = 0;
            let mut tanstable_a: &mut [u8] = &mut tans_data.a;
            let mut tanstable_b: &mut [u32] = &mut tans_data.b;

            for ri in range {
                let mut symbol = ri.symbol as i32;
                for _ in 0..ri.num {
                    bits.refill(core).at(self)?;

                    let nextra = cur_rice_ptr[0] as i32 + q;
                    cur_rice_ptr = &cur_rice_ptr[1..];
                    assert!(nextra <= 15);
                    let mut v = bits.read_bits_no_refill_zero(nextra) + (1 << nextra) - (1 << q);

                    let average_div4 = average >> 2;
                    let mut limit = 2 * average_div4;
                    if v <= limit {
                        v = average_div4 + ((v as u32 >> 1) as i32 ^ -(v & 1));
                    }
                    if limit > v {
                        limit = v;
                    }
                    v += 1;
                    average += limit - average_div4;
                    tanstable_a[0] = symbol as u8;
                    tanstable_b[0] = ((symbol << 16) + v) as u32;
                    if v == 1 {
                        tanstable_a = &mut tanstable_a[1..];
                    }
                    if v >= 2 {
                        tanstable_b = &mut tanstable_b[1..];
                    }
                    somesum += v;
                    symbol += 1;
                }
            }
            tans_data.a_used = (256 - tanstable_a.len()) as _;
            tans_data.b_used = (256 - tanstable_b.len()) as _;
            tans_data.assert_eq(somesum, l)?;

            Ok(tans_data)
        } else {
            let mut seen = [false; 256];
            let l = 1 << l_bits;

            let count = bits.read_bits_no_refill(3) + 1;

            let bits_per_sym = l_bits.ilog2() + 1;
            let max_delta_bits = bits.read_bits_no_refill(bits_per_sym as i32);

            assert_ne!(max_delta_bits, 0);
            assert!(max_delta_bits <= l_bits);

            let mut tanstable_a: &mut [u8] = &mut tans_data.a;
            let mut tanstable_b: &mut [u32] = &mut tans_data.b;

            let mut weight = 0;
            let mut total_weights = 0;

            for _ in 0..count {
                bits.refill(core).at(self)?;

                let sym = bits.read_bits_no_refill(8);
                assert!(!seen[sym as usize], "{}", sym);

                let delta = bits.read_bits_no_refill(max_delta_bits);

                weight += delta;

                assert_ne!(weight, 0);

                seen[sym as usize] = true;
                if weight == 1 {
                    tanstable_a[0] = sym as u8;
                    tanstable_a = &mut tanstable_a[1..];
                } else {
                    tanstable_b[0] = ((sym << 16) + weight) as u32;
                    tanstable_b = &mut tanstable_b[1..];
                }

                total_weights += weight;
            }

            bits.refill(core).at(self)?;

            let sym = bits.read_bits_no_refill(8);
            assert!(!seen[sym as usize], "{}", sym);

            assert!(l - total_weights >= weight);
            assert!(l - total_weights > 1);

            tanstable_b[0] = ((sym << 16) + (l - total_weights)) as u32;
            tanstable_b = &mut tanstable_b[1..];

            let a_used = 256 - tanstable_a.len();
            let b_used = 256 - tanstable_b.len();
            tans_data.a_used = a_used as _;
            tans_data.b_used = b_used as _;

            tans_data.a[..a_used].sort_unstable();
            tans_data.b[..b_used].sort_unstable();

            Ok(tans_data)
        }
    }
}

#[derive(Default, Copy, Clone)]
pub struct TansLutEnt {
    x: u32,
    bits_x: u8,
    symbol: u8,
    w: u16,
}

pub struct TansData {
    pub a_used: u32,
    pub b_used: u32,
    pub a: [u8; 256],
    pub b: [u32; 256],
}

impl ErrorContext for TansData {}
