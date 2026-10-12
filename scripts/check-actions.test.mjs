import test from 'node:test';
import assert from 'node:assert/strict';
import {checkActions,checkConcurrency,checkCredentialedDeploys,checkLockedInstalls,checkStackGating} from './check-actions.mjs';
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

test('runs slow workflows on pull requests into main only, and on every push to main',()=>{
  const edit="github.event.action != 'edited' || github.event.changes.base";
  const job=(name,uses,condition)=>`  ${name}:\n    if: \${{ ${condition} }}\n    uses: ./.github/workflows/${uses}\n`;
  const main='jobs:\n'+['run-headless-tests.yml','library-apple.yml','demo-macos.yml'].map((uses,i)=>`  j${i}:\n    uses: ./.github/workflows/${uses}\n`).join('');
  const pr=jobs=>[['on_pull_request.yml','on:\n  pull_request:\njobs:\n'+jobs],['on_main_push.yml',main]];
  checkStackGating(pr(job('tests','run-tests.yml',edit)+job('headless','run-headless-tests.yml',`(${edit}) && (github.base_ref == 'main')`)));
  assert.throws(()=>checkStackGating(pr(job('headless','run-headless-tests.yml',edit))),/headless is slow/);
  assert.throws(()=>checkStackGating(pr('  tests:\n    uses: ./.github/workflows/run-tests.yml\n')),/tests must skip edits/);
  assert.throws(()=>checkStackGating([pr('')[0],['on_main_push.yml','jobs:\n']]),/must run run-headless-tests\.yml/);
});

test('installs crates from crates.io with --locked, across continued lines',()=>{
  checkLockedInstalls([['a.yml','run: |\n  cargo binstall --no-confirm --locked \\\n    cargo-nextest\n  cargo binstall -V\n']]);
  checkLockedInstalls([['b.yml','run: cargo install --force --git "https://example.com/x" --rev abc tool\n']]);
  assert.throws(()=>checkLockedInstalls([['c.yml','run: cargo binstall --no-confirm cargo-nextest\n']]),/c\.yml must install with --locked/);
  assert.throws(()=>checkLockedInstalls([['d.yml','run: |\n  cargo install --debug \\\n    tool\n']]),/d\.yml/);
});

test('deploys to Cloudflare only where the credentials are set',()=>{
  const env="    env:\n      CAN_DEPLOY: ${{ secrets.CF_API_TOKEN != '' && secrets.CF_ACCOUNT_ID != '' }}\n";
  const step=gate=>`      - name: Deploy\n${gate}        uses: ./.github/actions/cloudflare-deploy\n`;
  checkCredentialedDeploys([['a.yml',`jobs:\n  deploy:\n${env}    steps:\n${step("        if: env.CAN_DEPLOY == 'true'\n")}`]]);
  assert.throws(()=>checkCredentialedDeploys([['b.yml',`jobs:\n  deploy:\n${env}    steps:\n${step('')}`]]),/b\.yml must deploy to Cloudflare only/);
  assert.throws(()=>checkCredentialedDeploys([['c.yml',`jobs:\n  deploy:\n    steps:\n${step("        if: env.CAN_DEPLOY == 'true'\n")}`]]),/c\.yml must set CAN_DEPLOY/);
});
