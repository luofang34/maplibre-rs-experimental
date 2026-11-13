import {readdirSync,readFileSync,statSync,writeFileSync} from 'node:fs';
import { dirname,join,posix,relative,resolve } from 'node:path';
import {fileURLToPath,pathToFileURL} from 'node:url';

const SCRIPT_DIR=dirname(fileURLToPath(import.meta.url));
export const REPO_ROOT=resolve(SCRIPT_DIR,'..');
export const DEFAULT_BASELINE_PATH=join(SCRIPT_DIR,'panic-baseline.json');

export function isTestOrGeneratedPath(relPath){
  const norm=relPath.split('\\').join('/');
  if(!norm.endsWith('.rs'))return true;
  if(/(^|\/)(tests|benches|examples|target|vendor)\//.test(norm))return true;
  if(/(^|\/)(build|tests|test)\.rs$/.test(norm))return true;
  return false;
}

// Replaces comments, character literals, and normal/raw string literals with spaces while
// preserving newlines, then blanks out `#[cfg(test)]` items.
export function stripNonCode(source){
  const len=source.length;
  const out=[];
  let i=0;
  const isIdentChar=ch=>ch!==undefined&&/[A-Za-z0-9_]/.test(ch);

  while(i<len){
    const ch=source[i];
    const next=source[i+1];

    // Line comment
    if(ch==='/'&&next==='/'){
      while(i<len&&source[i]!=='\n'){out.push(' ');i++;}
      continue;
    }

    // Nested block comment
    if(ch==='/'&&next==='*'){
      let depth=1;
      out.push(' ',' ');
      i+=2;
      while(i<len&&depth>0){
        if(source[i]==='/'&&source[i+1]==='*'){
          depth++;
          out.push(' ',' ');
          i+=2;
        }else if(source[i]==='*'&&source[i+1]==='/'){
          depth--;
          out.push(' ',' ');
          i+=2;
        }else{
          out.push(source[i]==='\n'?'\n':' ');
          i++;
        }
      }
      continue;
    }

    // Raw string literal: r"...", r#"..."#, br#"..."#, cr#"..."#
    if((ch==='r'||((ch==='b'||ch==='c')&&next==='r'))&&(i===0||!isIdentChar(source[i-1]))){
      let cursor=ch==='r'?i+1:i+2;
      let hashes=0;
      while(cursor<len&&source[cursor]==='#'){hashes++;cursor++;}
      if(source[cursor]==='"'){
        while(i<=cursor){out.push(' ');i++;}
        while(i<len){
          if(source[i]==='"'){
            let matched=0;
            while(matched<hashes&&source[i+1+matched]==='#')matched++;
            if(matched===hashes){
              for(let k=0;k<=hashes;k++){out.push(' ');i++;}
              break;
            }
          }
          out.push(source[i]==='\n'?'\n':' ');
          i++;
        }
        continue;
      }
    }

    // Normal, byte, or C string literal
    if(ch==='"'||((ch==='b'||ch==='c')&&next==='"'&&(i===0||!isIdentChar(source[i-1])))){
      if(ch!=='"'){out.push(' ');i++;}
      out.push(' ');
      i++;
      while(i<len){
        if(source[i]==='\\'){
          out.push(' ');
          i++;
          if(i<len){out.push(source[i]==='\n'?'\n':' ');i++;}
          continue;
        }
        if(source[i]==='"'){out.push(' ');i++;break;}
        out.push(source[i]==='\n'?'\n':' ');
        i++;
      }
      continue;
    }

    // Character literal ('x' or '\...') vs lifetime ('a)
    if(ch==="'"){
      if(next==='\\'){
        let close=i+2;
        while(close<len&&source[close]!=='\n'&&source[close]!=="'")close++;
        if(source[close]==="'"){
          while(i<=close){out.push(' ');i++;}
          continue;
        }
      }else if(i+2<len&&next!=="'"&&next!=='\n'&&source[i+2]==="'"){
        out.push(' ',' ',' ');
        i+=3;
        continue;
      }
    }

    out.push(ch);
    i++;
  }

  return stripCfgTestItems(out.join(''));
}

function stripCfgTestItems(code){
  const chars=code.split('');
  const len=chars.length;
  const attrRe=/#\[\s*cfg\s*\(\s*test\s*\)\s*\]/g;
  let match;
  while((match=attrRe.exec(code))!==null){
    let pos=match.index;
    for(let k=pos;k<pos+match[0].length;k++){
      if(chars[k]!=='\n')chars[k]=' ';
    }
    pos+=match[0].length;
    // Skip whitespace and any stacked #[...] attributes on the same test-only item.
    while(pos<len){
      while(pos<len&&/\s/.test(chars[pos]))pos++;
      if(chars[pos]==='#'&&chars[pos+1]==='['){
        let bracketDepth=0;
        while(pos<len){
          if(chars[pos]==='[')bracketDepth++;
          else if(chars[pos]===']'){
            bracketDepth--;
            if(bracketDepth===0){chars[pos]=' ';pos++;break;}
          }
          if(chars[pos]!=='\n')chars[pos]=' ';
          pos++;
        }
      }else{
        break;
      }
    }
    // Blank out the item up to its terminating `;` or balanced `{ ... }` body.
    let braceDepth=0;
    let seenBrace=false;
    while(pos<len){
      const c=chars[pos];
      if(c==='{'){braceDepth++;seenBrace=true;}
      else if(c==='}'){
        braceDepth--;
        if(seenBrace&&braceDepth===0){
          chars[pos]=' ';
          pos++;
          break;
        }
      }else if(!seenBrace&&c===';'){
        chars[pos]=' ';
        pos++;
        break;
      }
      if(c!=='\n')chars[pos]=' ';
      pos++;
    }
    attrRe.lastIndex=pos;
  }
  return chars.join('');
}

export function countPanicsInSource(source){
  const stripped=stripNonCode(source);
  const count=re=>(stripped.match(re)??[]).length;
  const unwrap=count(/\.\s*unwrap\s*\(/g);
  const expect=count(/\.\s*expect\s*\(/g);
  const panic=count(/\bpanic!\s*[\(\[\{]/g);
  const todo=count(/\btodo!\s*[\(\[\{]/g);
  const unimplemented=count(/\bunimplemented!\s*[\(\[\{]/g);
  return {
    total:unwrap+expect+panic+todo+unimplemented,
    unwrap,
    expect,
    panic,
    todo,
    unimplemented,
  };
}

function parseWorkspaceMembers(cargoToml){
  const membersMatch=cargoToml.match(/members\s*=\s*\[([\s\S]*?)\]/);
  if(!membersMatch)throw Error('Could not find workspace `members` in Cargo.toml');
  return Array.from(membersMatch[1].matchAll(/"([^"]+)"/g),m=>m[1]);
}

function collectRustFiles(dir,repoRoot,acc){
  let entries;
  try{entries=readdirSync(dir);}catch{return;}
  entries.sort();
  for(const name of entries){
    const full=join(dir,name);
    const stat=statSync(full);
    const rel=posix.normalize(relative(repoRoot,full).split('\\').join('/'));
    if(stat.isDirectory()){
      if(name==='target'||name==='tests'||name==='benches'||name==='examples'||name==='vendor')continue;
      collectRustFiles(full,repoRoot,acc);
    }else if(!isTestOrGeneratedPath(rel)){
      acc.push(full);
    }
  }
}

export function scanWorkspacePanics(repoRoot=REPO_ROOT){
  const members=parseWorkspaceMembers(readFileSync(join(repoRoot,'Cargo.toml'),'utf8'));
  const crates={};
  const fileDetails={};
  for(const member of members){
    const memberFiles=[];
    collectRustFiles(join(repoRoot,member,'src'),repoRoot,memberFiles);
    const summary={total:0,unwrap:0,expect:0,panic:0,todo:0,unimplemented:0};
    for(const file of memberFiles){
      const relPath=posix.normalize(relative(repoRoot,file).split('\\').join('/'));
      const counts=countPanicsInSource(readFileSync(file,'utf8'));
      summary.total+=counts.total;
      summary.unwrap+=counts.unwrap;
      summary.expect+=counts.expect;
      summary.panic+=counts.panic;
      summary.todo+=counts.todo;
      summary.unimplemented+=counts.unimplemented;
      if(counts.total>0){
        fileDetails[relPath]=counts;
      }
    }
    crates[member]=summary;
  }
  const files=Object.fromEntries(
    Object.entries(fileDetails)
      .filter(([,counts])=>counts.total>0)
      .sort(([a],[b])=>(a<b?-1:a>b?1:0))
      .map(([relPath,counts])=>[relPath,counts.total])
  );
  return {crates,files,fileDetails};
}

function hasReportShape(obj){
  return Boolean(
    obj&&
    typeof obj==='object'&&
    (('crates' in obj&&typeof obj.crates==='object')||('files' in obj&&typeof obj.files==='object'))
  );
}

export function checkPanicBaseline(current,baseline){
  const currentIsReport=hasReportShape(current);
  const baselineIsReport=hasReportShape(baseline);
  const currentCrates=currentIsReport?(current.crates??{}):(current??{});
  const baselineCrates=baselineIsReport?(baseline.crates??{}):(baseline??{});
  const currentFiles=currentIsReport?(current.fileDetails??current.files??null):null;
  const baselineFiles=baselineIsReport?(baseline.files??{}):{};

  const regressions=[];
  for(const [crate,counts] of Object.entries(currentCrates)){
    const allowed=baselineCrates[crate]?.total??0;
    if(counts.total>allowed){
      regressions.push(
        `crate ${crate}: ${allowed} -> ${counts.total} (+${counts.total-allowed}; unwrap=${counts.unwrap}, expect=${counts.expect}, panic=${counts.panic}, todo=${counts.todo}, unimplemented=${counts.unimplemented})`
      );
    }
  }
  if(currentFiles){
    const sortedFiles=Object.entries(currentFiles).sort(([a],[b])=>(a<b?-1:a>b?1:0));
    for(const [relPath,entry] of sortedFiles){
      const actual=typeof entry==='number'?entry:entry.total;
      const allowed=baselineFiles[relPath]??0;
      if(actual>allowed){
        if(typeof entry==='object'&&entry!==null){
          regressions.push(
            `file ${relPath}: ${allowed} -> ${actual} (+${actual-allowed}; unwrap=${entry.unwrap??0}, expect=${entry.expect??0}, panic=${entry.panic??0}, todo=${entry.todo??0}, unimplemented=${entry.unimplemented??0})`
          );
        }else{
          regressions.push(`file ${relPath}: ${allowed} -> ${actual} (+${actual-allowed})`);
        }
      }
    }
  }
  if(regressions.length){
    throw Error(
      `Non-test panic/unwrap count exceeded scripts/panic-baseline.json:\n${regressions.join('\n')}`
    );
  }
}

if(process.argv[1]&&import.meta.url===pathToFileURL(resolve(process.argv[1])).href){
  const writeMode=process.argv.includes('--write-baseline');
  const report=scanWorkspacePanics(REPO_ROOT);
  const {crates,files}=report;
  if(writeMode){
    const payload={
      description:'Monotonic ceiling on non-test .unwrap(), .expect(), panic!(), todo!(), and unimplemented!() sites per workspace crate and source file.',
      crates,
      files,
    };
    writeFileSync(DEFAULT_BASELINE_PATH,`${JSON.stringify(payload,null,2)}\n`);
    console.log(`Wrote panic baseline for ${Object.keys(crates).length} workspace crates and ${Object.keys(files).length} non-zero files to ${relative(REPO_ROOT,DEFAULT_BASELINE_PATH)}.`);
  }else{
    const baseline=JSON.parse(readFileSync(DEFAULT_BASELINE_PATH,'utf8'));
    checkPanicBaseline(report,baseline);
    const totalNow=Object.values(crates).reduce((sum,c)=>sum+c.total,0);
    const totalBase=Object.values(baseline.crates??{}).reduce((sum,c)=>sum+(c.total??0),0);
    const fileCountNow=Object.keys(files).length;
    const fileCountBase=Object.keys(baseline.files??{}).length;
    console.log(`Checked ${Object.keys(crates).length} workspace crates and ${fileCountNow}/${fileCountBase} baselined files (${totalNow} <= ${totalBase} total non-test panic sites).`);
  }
}
