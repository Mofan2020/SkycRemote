// 检查界面那头的 Annex-B 到 AVCC 转换，用的是真的编码器吐出来的帧。
//
// 跑法： node scripts/check-h264-conversion.mjs
//
// 这里只查结构：分了几片、参数集在不在、描述对不对。
// 能不能真的解出来，要到浏览器里才知道，那由 check-h264-decode.html 做。

import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const uiDir = new URL('../crates/skycremote-app/ui/', import.meta.url).pathname;
const source = readFileSync(uiDir + 'components/decoder.js', 'utf8');
const info = JSON.parse(readFileSync('/tmp/skyc-frame.json', 'utf8'));

const sandbox = {
  console,
  atob: (text) => Buffer.from(text, 'base64').toString('binary'),
  Uint8Array,
  Array,
  Math,
  JSON
};
sandbox.window = sandbox;
vm.createContext(sandbox);
vm.runInContext(source, sandbox);

const h264 = sandbox.SkycUI.h264;

let failures = 0;
function check(name, condition, detail = '') {
  if (condition) {
    console.log('  [通过] ' + name);
  } else {
    console.log('  [失败] ' + name + (detail ? ' —— ' + detail : ''));
    failures += 1;
  }
}

console.log('\n== 分片 ==');

const bytes = h264.fromText(info.base64);
check('base64 解出来的长度和帧一样', bytes.length === info.bytes,
  bytes.length + ' vs ' + info.bytes);

const pieces = h264.splitPieces(bytes);
check('分出了片', pieces.length > 0, pieces.length + ' 片');

const kinds = pieces.map((piece) => piece[0] & 0x1f);
console.log('  片的类型: ' + kinds.join(', '));
check('里面有参数集（7）', kinds.includes(7));
check('里面有另一个参数集（8）', kinds.includes(8));
check('里面有能独立解的画（5）', kinds.includes(5));
check('判断出这是关键帧', h264.isKeyPicture(pieces) === true);

const sps = pieces.find((piece) => (piece[0] & 0x1f) === 7);
const pps = pieces.find((piece) => (piece[0] & 0x1f) === 8);

console.log('\n== 描述 ==');

const desc = h264.description(sps, pps);
check('描述的第一个字节是 1', desc[0] === 1, String(desc[0]));
check('描述里带了参数集的长度', desc[5] === 0xe1);
check('描述的总长对得上', desc.length === 11 + sps.length + pps.length,
  desc.length + ' vs ' + (11 + sps.length + pps.length));
// 描述的排布：1 字节版本、3 字节流的信息、1 字节长度宽度、
// 1 字节参数集个数、2 字节长度、然后是参数集本身。
check('参数集被原样放进去了',
  Buffer.compare(Buffer.from(desc.slice(8, 8 + sps.length)), Buffer.from(sps)) === 0);
check('另一个参数集也被放进去了',
  Buffer.compare(
    Buffer.from(desc.slice(11 + sps.length, 11 + sps.length + pps.length)),
    Buffer.from(pps)) === 0);

const codec = h264.codecName(sps);
console.log('  解码器名字: ' + codec);
check('名字是 avc1. 开头的', codec.startsWith('avc1.') && codec.length === 11, codec);
check('名字里的数字取自参数集',
  codec === 'avc1.' +
    [sps[1], sps[2], sps[3]].map((n) => n.toString(16).padStart(2, '0')).join(''));

console.log('\n== AVCC ==');

const pictures = pieces.filter((piece) => {
  const kind = piece[0] & 0x1f;
  return kind !== 7 && kind !== 8;
});
const avcc = h264.toAvcc(pictures);
let expected = 0;
for (const picture of pictures) expected += 4 + picture.length;
check('长度对得上', avcc.length === expected, avcc.length + ' vs ' + expected);

let lengthsRight = true;
let at = 0;
for (const picture of pictures) {
  const length =
    (avcc[at] << 24) | (avcc[at + 1] << 16) | (avcc[at + 2] << 8) | avcc[at + 3];
  if (length !== picture.length) lengthsRight = false;
  const same = Buffer.compare(
    Buffer.from(avcc.slice(at + 4, at + 4 + length)),
    Buffer.from(picture)) === 0;
  if (!same) lengthsRight = false;
  at += 4 + length;
}
check('每片的长度前缀和内容都对', lengthsRight);

console.log(failures === 0 ? '\n结论: 全部通过' : '\n结论: ' + failures + ' 项失败');
process.exit(failures === 0 ? 0 : 1);
