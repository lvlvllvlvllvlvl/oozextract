import {Extractor} from "oozextract";
import {decompressUnsafe} from "ooz-wasm";
import {readdirSync, readFileSync} from "node:fs";
import path from "node:path";
import {assert, describe, test} from "vitest";

const extractor = Extractor.new();

const files = readdirSync("../testdata");
const algorithms: string[] = [];

for (const file of files) {
    const data = readFileSync(path.join("..", "testdata", file));
    const len = data[4] == 0x8C ? data.readInt32LE(0) : Number(data.readBigInt64LE(0));
    const input = new Uint8Array(data).slice(data[4] == 0x8C ? 4 : 8);
    const [basename, ext] = file.split(".")
    const expected = readFileSync(path.join("..", "verify", basename));
    if (!algorithms.includes(ext)) {
        algorithms.push(ext);
        describe(file, () => {
            test("oozextract", () => {
                const out = extractor.extract(input, len);
                assert.equal(out.length, expected.length);
                for (let i = 0; i < expected.length; i++) {
                    assert.equal(out[i], expected[i], i.toString());
                }
            });
            test("ooz-wasm", () => {
                const out = decompressUnsafe(input, len);
                assert.equal(out.length, expected.length);
                for (let i = 0; i < expected.length; i++) {
                    assert.equal(out[i], expected[i], i.toString());
                }
            });
        });
    }
}
