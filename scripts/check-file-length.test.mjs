import test from 'node:test';
import assert from 'node:assert/strict';
import {checkFileLengths,lineCount} from './check-file-length.mjs';

test('counts lines with or without a final newline',()=>{
  assert.equal(lineCount(''),0);
  assert.equal(lineCount('a\nb\n'),2);
  assert.equal(lineCount('a\nb'),2);
});

test('rejects a Rust file that passes the limit or grows past it, and allows shrinking one',()=>{
  checkFileLengths([['a.rs',undefined,500],['b.rs',800,800],['c.rs',800,640],['d.md',undefined,900]]);
  assert.throws(()=>checkFileLengths([['e.rs',undefined,501]]),/e\.rs: new -> 501/);
  assert.throws(()=>checkFileLengths([['f.rs',480,520]]),/f\.rs: 480 -> 520/);
  assert.throws(()=>checkFileLengths([['g.rs',700,701]]),/g\.rs: 700 -> 701/);
});
