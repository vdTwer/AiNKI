// Files the tests upload: a small PNG, and an Anki package built on the
// spot (a real SQLite collection in a zip), so no binary fixtures live in
// the repository.
import { DatabaseSync } from 'node:sqlite';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { deflateRawSync, deflateSync } from 'node:zlib';

const CRC_TABLE = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});

function crc32(buf: Buffer): number {
  let c = 0xffffffff;
  for (const b of buf) c = CRC_TABLE[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function chunk(type: string, data: Buffer): Buffer {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, 'ascii'), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([len, body, crc]);
}

/** A width x height RGB PNG with a simple gradient. */
export function png(width = 24, height = 16): Buffer {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 2; // truecolor
  const rows = [];
  for (let y = 0; y < height; y++) {
    const row = Buffer.alloc(1 + width * 3);
    for (let x = 0; x < width; x++) row.set([x * 10, y * 15, 120], 1 + x * 3);
    rows.push(row);
  }
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    chunk('IDAT', deflateSync(Buffer.concat(rows))),
    chunk('IEND', Buffer.alloc(0)),
  ]);
}

/** A zip archive (deflate) of the given files. */
export function zip(files: Record<string, Buffer>): Buffer {
  const locals: Buffer[] = [];
  const centrals: Buffer[] = [];
  let offset = 0;
  for (const [name, data] of Object.entries(files)) {
    const nameBuf = Buffer.from(name, 'utf8');
    const packed = deflateRawSync(data);
    const crc = crc32(data);
    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x04034b50, 0);
    local.writeUInt16LE(20, 4);
    local.writeUInt16LE(0, 6);
    local.writeUInt16LE(8, 8); // deflate
    local.writeUInt32LE(crc, 14);
    local.writeUInt32LE(packed.length, 18);
    local.writeUInt32LE(data.length, 22);
    local.writeUInt16LE(nameBuf.length, 26);
    locals.push(local, nameBuf, packed);
    const central = Buffer.alloc(46);
    central.writeUInt32LE(0x02014b50, 0);
    central.writeUInt16LE(20, 4);
    central.writeUInt16LE(20, 6);
    central.writeUInt16LE(8, 10);
    central.writeUInt32LE(crc, 16);
    central.writeUInt32LE(packed.length, 20);
    central.writeUInt32LE(data.length, 24);
    central.writeUInt16LE(nameBuf.length, 28);
    central.writeUInt32LE(offset, 42);
    centrals.push(central, nameBuf);
    offset += local.length + nameBuf.length + packed.length;
  }
  const dir = Buffer.concat(centrals);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(Object.keys(files).length, 8);
  end.writeUInt16LE(Object.keys(files).length, 10);
  end.writeUInt32LE(dir.length, 12);
  end.writeUInt32LE(offset, 16);
  return Buffer.concat([...locals, dir, end]);
}

export interface AnkiNote {
  deck: string;
  fields: [string, string];
  /** Card type in Anki: 0 new, 2 review (then ivl days, due in days from now). */
  review?: { ivl: number; dueInDays: number };
  suspended?: boolean;
}

/** A legacy .apkg (collection.anki2 + media) with the given notes. */
export function apkg(notes: AnkiNote[], media: Record<string, Buffer> = {}): Buffer {
  const file = path.join(fs.mkdtempSync(path.join(os.tmpdir(), 'apkg-')), 'collection.anki2');
  const db = new DatabaseSync(file);
  db.exec(`
    CREATE TABLE col (id INTEGER PRIMARY KEY, crt INTEGER, decks TEXT);
    CREATE TABLE notes (id INTEGER PRIMARY KEY, flds TEXT);
    CREATE TABLE cards (id INTEGER PRIMARY KEY, nid INTEGER, did INTEGER, ord INTEGER, type INTEGER,
      queue INTEGER, due INTEGER, ivl INTEGER, factor INTEGER, reps INTEGER, lapses INTEGER,
      odue INTEGER DEFAULT 0, odid INTEGER DEFAULT 0);
    CREATE TABLE revlog (id INTEGER PRIMARY KEY, cid INTEGER, ease INTEGER, type INTEGER);
  `);
  const now = Math.floor(Date.now() / 1000);
  const crt = now - 100 * 86_400;
  const deckIds = new Map<string, number>();
  for (const n of notes) if (!deckIds.has(n.deck)) deckIds.set(n.deck, 100 + deckIds.size);
  const decks = Object.fromEntries([...deckIds].map(([name, id]) => [String(id), { name }]));
  db.prepare('INSERT INTO col VALUES (1, ?, ?)').run(crt, JSON.stringify(decks));
  notes.forEach((n, i) => {
    db.prepare('INSERT INTO notes VALUES (?, ?)').run(i + 1, n.fields.join('\x1f'));
    const today = Math.floor((now - crt) / 86_400);
    db.prepare('INSERT INTO cards (id, nid, did, ord, type, queue, due, ivl, factor, reps, lapses) VALUES (?, ?, ?, 0, ?, ?, ?, ?, ?, 0, 0)').run(
      1000 + i,
      i + 1,
      deckIds.get(n.deck)!,
      n.review ? 2 : 0,
      n.suspended ? -1 : n.review ? 2 : 0,
      n.review ? today + n.review.dueInDays : i,
      n.review?.ivl ?? 0,
      n.review ? 2500 : 0,
    );
  });
  db.close();
  const names = Object.keys(media);
  const files: Record<string, Buffer> = {
    'collection.anki2': fs.readFileSync(file),
    media: Buffer.from(JSON.stringify(Object.fromEntries(names.map((n, i) => [String(i), n])))),
  };
  names.forEach((n, i) => (files[String(i)] = media[n]));
  return zip(files);
}
