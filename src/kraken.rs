



// Kraken decompression happens in two phases, first one decodes
// all the literals and copy lengths using huffman and second
// phase runs the copy loop. This holds the tables needed by stage 2.
struct KrakenLzTable {
    // Stream of (literal, match) pairs. The flag u8 contains
    // the length of the match, the length of the literal and whether
    // to use a recent offset.
    cmd_stream: Vec<u8>,

    // Holds the actual distances in case we're not using a recent
    // offset.
    offs_stream: Vec<i32>,

    // Holds the sequence of literals. All literal copying happens from
    // here.
    lit_stream: Vec<u8>,

    // Holds the lengths that do not fit in the flag stream. Both literal
    // lengths and match length are stored in the same array.
    len_stream: Vec<i32>,
}

// Mermaid/Selkie decompression also happens in two phases, just like in Kraken,
// but the match copier works differently.
// Both Mermaid and Selkie use the same on-disk format, only the compressor
// differs.
struct MermaidLzTable {
    // Flag stream. Format of flags:
    // Read flagbyte from |cmd_stream|
    // If flagbyte >= 24:
    //   flagbyte & 0x80 == 0 : Read from |off16_stream| into |recent_offs|.
    //                   != 0 : Don't read offset.
    //   flagbyte & 7 = Number of literals to copy first from |lit_stream|.
    //   (flagbyte >> 3) & 0xF = Number of bytes to copy from |recent_offs|.
    //
    //  If flagbyte == 0 :
    //    Read u8 L from |length_stream|
    //    If L > 251: L += 4 * Read word from |length_stream|
    //    L += 64
    //    Copy L bytes from |lit_stream|.
    //
    //  If flagbyte == 1 :
    //    Read u8 L from |length_stream|
    //    If L > 251: L += 4 * Read word from |length_stream|
    //    L += 91
    //    Copy L bytes from match pointed by next offset from |off16_stream|
    //
    //  If flagbyte == 2 :
    //    Read u8 L from |length_stream|
    //    If L > 251: L += 4 * Read word from |length_stream|
    //    L += 29
    //    Copy L bytes from match pointed by next offset from |off32_stream|,
    //    relative to start of block.
    //    Then prefetch |off32_stream[3]|
    //
    //  If flagbyte > 2:
    //    L = flagbyte + 5
    //    Copy L bytes from match pointed by next offset from |off32_stream|,
    //    relative to start of block.
    //    Then prefetch |off32_stream[3]|
    cmd_stream: Vec<u8>,

    // Length stream
    length_stream: Vec<u8>,

    // Literal stream
    lit_stream: Vec<u8>,

    // Near offsets
    off16_stream: Vec<u16>,

    // Far offsets for current chunk
    off32_stream: Vec<u32>,

    // Holds the offsets for the two chunks
    off32_stream_1: Vec<u32>,
    off32_stream_2: Vec<u32>,
    off32_size_1: u32,
    off32_size_2: u32,

    // Flag offsets for next 64k chunk.
    cmd_stream_2_offs: u32,
    cmd_stream_2_offs_end: u32,
}

struct BitReader {
    // |p| holds the current u8 and |p_end| the end of the buffer.
    p: Vec<u8>,
    p_end: Vec<u8>,
    // Bits accumulated so far
    bits: u32,
    // Next u8 will end up in the |bitpos| position in |bits|.
    bitpos: i32,
}

struct HuffRevLut {
    bits2len: [u8; 2048],
    bits2sym: [u8; 2048],
}

struct HuffReader {
    // Array to hold the output of the huffman read array operation
    output: Vec<u8>,
    output_end: Vec<u8>,
    // We decode three parallel streams, two forwards, |src| and |src_mid|
    // while |src_end| is decoded backwards.
    src: Vec<u8>,
    src_mid: Vec<u8>,
    src_end: Vec<u8>,
    src_mid_org: Vec<u8>,
    src_bitpos: i32,
    src_mid_bitpos: i32,
    src_end_bitpos: i32,
    src_bits: u32,
    src_mid_bits: u32,
    src_end_bits: u32,
}

fn Max<T: PartialOrd>(a: T, b: T) -> T {
    if a > b {
        a
    } else {
        b
    }
}

fn Min<T: PartialOrd>(a: T, b: T) -> T {
    if a < b {
        a
    } else {
        b
    }
}

fn BSR(x: u32) -> u32 {
    x.leading_zeros() ^ 31
}

fn BSF(x: u32) -> u32 {
    x.trailing_zeros()
}
