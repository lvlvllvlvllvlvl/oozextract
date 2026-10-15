# oozextract

Open source Kraken / Mermaid / Selkie / Leviathan / LZNA / Bitknit decompressor

```
it's probably fuzz safe, you can trust the input if you like
```

## Restart regression fixtures

`testdata/kraken-restart.kraken` contains two independently compressed 256 KiB
buffers. Each input byte is `(i * 17 + i / 256 + j) % 251`, with `i` starting at
zero in each buffer and `j = 0, 1`. `kraken-restart-history.kraken` uses the same
pattern with buffer lengths 256 KiB and 512 KiB, so the third quantum continues
the history established by the second quantum's restart. Both were compressed
with Oodle 2.6, Kraken, level 4; their first eight bytes hold the total decoded
length in little-endian order. The corresponding synthetic decoded bytes are
in `verify/`. The existing slice, reader, and async corpus tests exercise these
fixtures without requiring a native compressor or decoder.
