import {execFileSync} from 'node:child_process';
import {readFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';

// A Rust file past this many lines is split into submodules. One already past it may be edited
// without being split, as long as the edit does not grow it.
export const LINE_LIMIT=500;

export function lineCount(text){
  return text.length===0?0:text.split('\n').length-(text.endsWith('\n')?1:0);
}

// Each change is [path, lines at the base or undefined for a new file, lines now].
export function checkFileLengths(changes){
  const grown=changes.filter(([path,before,after])=>path.endsWith('.rs')&&after>LINE_LIMIT&&(before===undefined||after>before));
  if(grown.length)throw Error(`Split these Rust files under ${LINE_LIMIT} lines, or leave them no longer than at the base:\n${grown.map(([path,before,after])=>`${path}: ${before??'new'} -> ${after}`).join('\n')}`);
}

if(process.argv[1]&&import.meta.url===pathToFileURL(resolve(process.argv[1])).href){
  const base=process.argv[2]??'HEAD';
  const git=args=>execFileSync('git',args,{encoding:'utf8',maxBuffer:1<<28});
  const paths=git(['diff','--no-renames','--name-only','--diff-filter=AM','-z',base,'--']).split('\0').filter(Boolean);
  const changes=paths.map(path=>{
    let before;
    try{before=lineCount(git(['show',`${base}:${path}`]))}catch{before=undefined}
    return [path,before,lineCount(readFileSync(path,'utf8'))];
  });
  checkFileLengths(changes);
  console.log(`Checked ${changes.length} changed files against the ${LINE_LIMIT}-line limit.`);
}
