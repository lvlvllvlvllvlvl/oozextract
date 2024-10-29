import {Extractor} from "oozextract";
import {decompressUnsafe} from "ooz-wasm";
import {readdirSync, readFileSync} from "node:fs";
import path from "node:path";
import {bench, describe} from "vitest";

const extractor = Extractor.new();

const files = readdirSync("../testdata");

for (const file of files) {
    const data = readFileSync(path.join("..", "testdata", file));
    const len = data[4] == 0x8C ? data.readInt32LE(0) : Number(data.readBigInt64LE(0));
    const input = new Uint8Array(data).slice(data[4] == 0x8C ? 4 : 8);
    describe(file, () => {
        bench("oozextract", () => {
            extractor.extract(input, len);
        })
        bench("ooz-wasm", () => {
            decompressUnsafe(input, len);
        })
    })
}
