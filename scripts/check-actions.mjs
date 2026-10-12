import {readFile,readdir} from 'node:fs/promises';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';

export function actionUses(text){
  return [...text.matchAll(/^\s*(?:-\s*)?uses:\s*["']?([^\s"'#]+)["']?/gm)].map(match=>match[1]);
}

export async function checkActions(workflows,load){
  const checked=new Set();
  async function visit(action,chain){
    if(checked.has(action))return;
    if(!action.startsWith('./.github/')&&!/^[\w.-]+\/[\w./-]+@[a-f0-9]{40}$/.test(action))throw Error(`Action must use an immutable revision: ${action}`);
    checked.add(action);
    const text=await load(action),runtime=text.match(/^\s*using:\s*["']?([\w]+)["']?/m)?.[1];
    const path=[...chain,action];
    if(runtime==='composite'||(action.startsWith('./.github/workflows/')&&/workflow_call:/.test(text))){
      for(const child of actionUses(text))await visit(child,path);
    }else if(runtime!=='node24'&&runtime!=='docker'){
      throw Error(`Unsupported action runtime ${runtime}: ${path.join(' -> ')}`);
    }
  }
  for(const text of workflows)for(const action of actionUses(text))await visit(action,[]);
  return [...checked];
}

// Merging a stack of pull requests pushes main once per pull request and rebases each one
// left; a workflow those start must cancel the run a newer push supersedes.
export function checkConcurrency(workflows){
  for(const [name,text] of workflows){
    const started=/^  pull_request:/m.test(text)||/^  push:\n(?:    .*\n)*?    branches:/m.test(text);
    if(started&&!/^  cancel-in-progress: true$/m.test(text))throw Error(`${name} must cancel superseded runs (concurrency with cancel-in-progress)`);
  }
}

// Each pull request of a stack runs on everything under it. The slow workflows run only on a
// pull request into main, as each reaches it, and on every push to main; an edit that leaves
// the base alone runs nothing.
export const SLOW_WORKFLOWS=['run-headless-tests.yml','library-apple.yml','demo-macos.yml'];
function jobBlocks(text){
  const jobs=text.slice(text.search(/^jobs:$/m));
  return jobs.split(/^(?=  [a-z][\w-]*:$)/m).slice(1).map(block=>[block.match(/^  ([a-z][\w-]*):/)[1],block]);
}
export function checkStackGating(workflows){
  const files=new Map(workflows);
  const pr=files.get('on_pull_request.yml'),main=files.get('on_main_push.yml');
  if(pr===undefined||main===undefined)throw Error('on_pull_request.yml and on_main_push.yml are required');
  for(const [job,block] of jobBlocks(pr)){
    const condition=block.match(/^    if: (.*)$/m)?.[1]??'';
    if(!condition.includes("github.event.action != 'edited' || github.event.changes.base"))throw Error(`on_pull_request.yml job ${job} must skip edits that keep the base`);
    const slow=SLOW_WORKFLOWS.some(name=>block.includes(`./.github/workflows/${name}`));
    if(slow&&!condition.includes("github.base_ref == 'main'"))throw Error(`on_pull_request.yml job ${job} is slow and must run only on pull requests into main`);
  }
  for(const name of SLOW_WORKFLOWS){
    if(!main.includes(`./.github/workflows/${name}`))throw Error(`on_main_push.yml must run ${name}`);
  }
}

// A crate installed from crates.io without --locked resolves its dependencies afresh, and
// some, such as cargo-nextest, refuse to build that way; binstall falls back to building
// from source whenever it cannot reach a crate's release in time.
export function checkLockedInstalls(files){
  for(const [name,text] of files){
    const lines=text.replace(/\\\n\s*/g,' ').split('\n');
    for(const line of lines){
      const command=line.match(/\bcargo b?install\b.*/)?.[0];
      if(command===undefined||/\s(?:-V|--version|--help)\b/.test(command)||/\s--git\s/.test(command))continue;
      if(!/\s--locked\b/.test(command))throw Error(`${name} must install with --locked: ${command.trim()}`);
    }
  }
}

async function loadAction(action){
  if(action.startsWith('./.github/')){
    if(action.includes('..'))throw Error('Invalid local action path');
    const path=action.startsWith('./.github/workflows/')?action:`${action}/action.yml`;
    return readFile(new URL('../'+path,import.meta.url),'utf8');
  }
  const [location,ref]=action.split('@'),[owner,repo,...directory]=location.split('/');
  const base=`https://raw.githubusercontent.com/${owner}/${repo}/${ref}/${directory.length?directory.join('/')+'/':''}`;
  for(const name of ['action.yml','action.yaml']){
    const response=await fetch(base+name,{signal:AbortSignal.timeout(30_000)});
    if(response.status===404)continue;
    if(!response.ok)throw Error(`Read ${action}: HTTP ${response.status}`);
    return response.text();
  }
  throw Error(`Missing action metadata: ${action}`);
}

if(process.argv[1]&&import.meta.url===pathToFileURL(resolve(process.argv[1])).href){
  const directory=new URL('../.github/workflows/',import.meta.url);
  const files=(await readdir(directory)).filter(name=>/\.ya?ml$/.test(name));
  const workflows=await Promise.all(files.map(name=>readFile(new URL(name,directory),'utf8')));
  checkConcurrency(files.map((name,index)=>[name,workflows[index]]));
  checkStackGating(files.map((name,index)=>[name,workflows[index]]));
  const actionsDirectory=new URL('../.github/actions/',import.meta.url);
  const local=await Promise.all((await readdir(actionsDirectory)).map(async name=>[`actions/${name}`,await readFile(new URL(`${name}/action.yml`,actionsDirectory),'utf8')]));
  checkLockedInstalls([...files.map((name,index)=>[name,workflows[index]]),...local]);
  const actions=await checkActions(workflows,loadAction);
  console.log(`Verified ${actions.length} direct and transitive actions: Node.js 24, composite, or Docker.`);
}
