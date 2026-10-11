import test from 'node:test';
import assert from 'node:assert/strict';
import {checkActions,checkConcurrency} from './check-actions.mjs';
const action=name=>`actions/${name}@${'a'.repeat(40)}`;
const uses=name=>`- uses: ${action(name)} # pinned`;
test('follows composite dependencies and rejects Node.js 20 with its chain',async()=>{
 const metadata={pages:`runs:\n  using: composite\n  steps:\n    ${uses('upload')}`,upload:"runs:\n  using: 'node20'"};
 await assert.rejects(checkActions([uses('pages')],async key=>metadata[key.split('/')[1].split('@')[0]]),/node20: actions\/pages@.* -> actions\/upload@/);
});
test('accepts Node.js 24 and visits a shared dependency once',async()=>{
 const seen=[];
 const result=await checkActions([uses('pages'),uses('upload')],async key=>{seen.push(key);return key===action('pages')?`using: composite\n${uses('upload')}`:'using: node24';});
 assert.deepEqual(result,[action('pages'),action('upload')]);assert.equal(seen.length,2);
});
test('rejects mutable references and unknown runtimes',async()=>{
 await assert.rejects(checkActions(['uses: actions/checkout@v7'],async()=>''),/immutable/);
 await assert.rejects(checkActions([uses('empty')],async()=>''),/Unsupported action runtime/);
});
test('retains metadata download failures',async()=>{
 await assert.rejects(checkActions([uses('pages')],async()=>{throw Error('HTTP 503')}),/HTTP 503/);
});

test('checks local reusable workflows and composite action dependencies',async()=>{
 const metadata={
  './.github/workflows/build.yml':"on:\n  workflow_call:\njobs:\n  build:\n    steps:\n      - uses: ./.github/actions/setup",
  './.github/actions/setup':`using: composite\n${uses('checkout')}`,
  [action('checkout')]:"using: node20",
 };
 await assert.rejects(checkActions(['uses: ./.github/workflows/build.yml'],async key=>metadata[key]),/node20:.*workflows\/build.yml ->.*actions\/setup ->.*checkout/);
});

test('requires pull request and branch push workflows to cancel superseded runs',()=>{
  const cancels='concurrency:\n  group: x\n  cancel-in-progress: true\n';
  assert.throws(()=>checkConcurrency([['pr.yml','on:\n  pull_request:\njobs: {}\n']]),/pr\.yml must cancel/);
  assert.throws(()=>checkConcurrency([['main.yml','on:\n  push:\n    branches: [main]\n']]),/main\.yml/);
  checkConcurrency([['pr.yml','on:\n  pull_request:\n'+cancels]]);
  checkConcurrency([['release.yml','on:\n  push:\n    tags: [\"v*\"]\n']]);
});
