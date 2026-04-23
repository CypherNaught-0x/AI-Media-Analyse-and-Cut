import { describe, it, expect } from 'vitest';
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join, resolve } from 'node:path';

// Contract test: every `invoke("cmd", { ... })` call in the frontend must
// pass all arguments required by the matching `#[tauri::command]` function
// in src-tauri. This prevents regressions like the missing `fastMode` arg
// that was silently accepted by TypeScript but rejected at runtime.

const REPO_ROOT = resolve(__dirname, '..', '..');
const TAURI_SRC = join(REPO_ROOT, 'src-tauri', 'src');
const FRONTEND_SRC = join(REPO_ROOT, 'src');

// Tauri-injected args — never sent from the frontend.
const INJECTED_ARG_TYPES = /^(tauri::Window|tauri::AppHandle|tauri::State<.*>|Window|AppHandle|State<.*>)$/;

type RustCommand = {
  name: string;
  requiredArgs: string[]; // camelCase, matching what Tauri expects on the JS side
  file: string;
};

type InvokeCall = {
  command: string;
  passedArgs: string[];
  file: string;
  line: number;
};

function walk(dir: string, exts: string[]): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(dir)) {
    const p = join(dir, entry);
    const s = statSync(p);
    if (s.isDirectory()) {
      if (entry === 'node_modules' || entry === 'target' || entry === 'dist') continue;
      out.push(...walk(p, exts));
    } else if (exts.some(e => entry.endsWith(e))) {
      out.push(p);
    }
  }
  return out;
}

function snakeToCamel(s: string): string {
  return s.replace(/_([a-z0-9])/g, (_, c) => c.toUpperCase());
}

function stripOuterWhitespace(s: string): string {
  return s.trim();
}

// Split top-level comma-separated params, respecting <>, (), [] nesting.
function splitParams(sig: string): string[] {
  const out: string[] = [];
  let depth = 0;
  let start = 0;
  for (let i = 0; i < sig.length; i++) {
    const c = sig[i];
    if (c === '<' || c === '(' || c === '[') depth++;
    else if (c === '>' || c === ')' || c === ']') depth--;
    else if (c === ',' && depth === 0) {
      out.push(sig.slice(start, i));
      start = i + 1;
    }
  }
  const tail = sig.slice(start);
  if (tail.trim()) out.push(tail);
  return out.map(stripOuterWhitespace).filter(Boolean);
}

function parseRustCommands(): RustCommand[] {
  const files = walk(TAURI_SRC, ['.rs']);
  const commands: RustCommand[] = [];
  // Match: #[tauri::command] then an optional attribute line, then (async )? fn NAME ( ... )
  // We capture the signature between the matching parens. Because Rust sigs may
  // span many lines and contain nested <> we use a paren-depth scanner.
  for (const file of files) {
    const src = readFileSync(file, 'utf8');
    const attrRegex = /#\[tauri::command\]/g;
    let m: RegExpExecArray | null;
    while ((m = attrRegex.exec(src)) !== null) {
      // Find "fn NAME(" after the attribute, skipping other attributes/visibility/async.
      const rest = src.slice(m.index);
      const fnMatch = rest.match(/fn\s+([a-zA-Z_][a-zA-Z0-9_]*)\s*\(/);
      if (!fnMatch) continue;
      const name = fnMatch[1];
      const parenStart = rest.indexOf('(', rest.indexOf(fnMatch[0]));
      // Scan for matching close paren.
      let depth = 0;
      let end = -1;
      for (let i = parenStart; i < rest.length; i++) {
        const c = rest[i];
        if (c === '(') depth++;
        else if (c === ')') {
          depth--;
          if (depth === 0) {
            end = i;
            break;
          }
        }
      }
      if (end === -1) continue;
      const sig = rest.slice(parenStart + 1, end);
      const params = splitParams(sig);
      const requiredArgs: string[] = [];
      for (const p of params) {
        // Each param: "name: Type" (possibly "mut name: Type"). Strip attrs like #[allow(...)].
        const cleaned = p.replace(/#\[[^\]]+\]/g, '').trim();
        const colonIdx = cleaned.indexOf(':');
        if (colonIdx === -1) continue;
        let argName = cleaned.slice(0, colonIdx).trim();
        const argType = cleaned.slice(colonIdx + 1).trim();
        argName = argName.replace(/^mut\s+/, '');
        // Skip Tauri-injected args.
        if (INJECTED_ARG_TYPES.test(argType)) continue;
        // Leading `_` in Rust means unused; Tauri strips it for the JS name.
        const jsName = snakeToCamel(argName.replace(/^_+/, ''));
        // Skip Option<...> args — they're optional on the JS side.
        if (/^Option\s*<[\s\S]*>$/.test(argType)) continue;
        requiredArgs.push(jsName);
      }
      commands.push({ name, requiredArgs, file });
    }
  }
  return commands;
}

function parseInvokeCalls(): InvokeCall[] {
  const files = walk(FRONTEND_SRC, ['.ts', '.vue']);
  const calls: InvokeCall[] = [];
  for (const file of files) {
    if (file.includes('__tests__') || file.endsWith('.spec.ts') || file.endsWith('.test.ts')) continue;
    const src = readFileSync(file, 'utf8');
    // Match invoke<Generic>?("cmd", { ... }) or invoke<Generic>?('cmd', { ... })
    // Handle multi-line object literals by tracking brace depth.
    const re = /\binvoke\s*(?:<[^>]*>)?\s*\(\s*["']([a-zA-Z_][a-zA-Z0-9_]*)["']\s*,\s*\{/g;
    let m: RegExpExecArray | null;
    while ((m = re.exec(src)) !== null) {
      const command = m[1];
      const objStart = m.index + m[0].length - 1; // position of `{`
      // Scan for matching close brace.
      let depth = 0;
      let end = -1;
      let inString: string | null = null;
      let prev = '';
      for (let i = objStart; i < src.length; i++) {
        const c = src[i];
        if (inString) {
          if (c === inString && prev !== '\\') inString = null;
        } else if (c === '"' || c === "'" || c === '`') {
          inString = c;
        } else if (c === '{') depth++;
        else if (c === '}') {
          depth--;
          if (depth === 0) {
            end = i;
            break;
          }
        }
        prev = c;
      }
      if (end === -1) continue;
      const objBody = src.slice(objStart + 1, end);
      const passedArgs = extractObjectKeys(objBody);
      const line = src.slice(0, m.index).split('\n').length;
      calls.push({ command, passedArgs, file, line });
    }
  }
  return calls;
}

function extractObjectKeys(body: string): string[] {
  // Strip nested braces/brackets/parens so we only see top-level keys.
  let out = '';
  let depth = 0;
  let inString: string | null = null;
  let prev = '';
  for (let i = 0; i < body.length; i++) {
    const c = body[i];
    if (inString) {
      if (c === inString && prev !== '\\') inString = null;
      prev = c;
      continue;
    }
    if (c === '"' || c === "'" || c === '`') {
      inString = c;
      prev = c;
      continue;
    }
    if (c === '{' || c === '(' || c === '[') depth++;
    else if (c === '}' || c === ')' || c === ']') depth--;
    if (depth === 0) out += c;
    prev = c;
  }
  // Strip line/block comments.
  out = out.replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/.*$/gm, '');
  const keys: string[] = [];
  // Match `foo:` or `"foo":` or shorthand `foo,` / `foo\s*$`.
  const parts = out.split(',');
  for (const part of parts) {
    const trimmed = part.trim();
    if (!trimmed) continue;
    // Skip spread: `...foo` — we can't statically resolve keys inside.
    if (trimmed.startsWith('...')) {
      keys.push('__spread__');
      continue;
    }
    const kv = trimmed.match(/^(?:"([^"]+)"|'([^']+)'|([a-zA-Z_][a-zA-Z0-9_]*))\s*(?::|$)/);
    if (kv) {
      keys.push(kv[1] ?? kv[2] ?? kv[3]);
    }
  }
  return keys;
}

const commands = parseRustCommands();
const commandsByName = new Map(commands.map(c => [c.name, c]));
const invokeCalls = parseInvokeCalls();

describe('Tauri invoke contract', () => {
  it('parses at least one Rust command and one invoke call', () => {
    expect(commands.length).toBeGreaterThan(0);
    expect(invokeCalls.length).toBeGreaterThan(0);
  });

  it('every invoke() call targets a registered Tauri command', () => {
    const unknown = invokeCalls.filter(c => !commandsByName.has(c.command));
    expect(
      unknown,
      `Unknown commands:\n${unknown.map(c => `  ${c.command} at ${c.file}:${c.line}`).join('\n')}`
    ).toEqual([]);
  });

  it('every invoke() call passes all required args', () => {
    const violations: string[] = [];
    for (const call of invokeCalls) {
      const cmd = commandsByName.get(call.command);
      if (!cmd) continue; // reported by the other test
      // If the call uses spread, we can't statically prove it — skip.
      if (call.passedArgs.includes('__spread__')) continue;
      const missing = cmd.requiredArgs.filter(a => !call.passedArgs.includes(a));
      if (missing.length > 0) {
        violations.push(
          `  ${call.command} at ${call.file}:${call.line} missing: ${missing.join(', ')} ` +
            `(required: ${cmd.requiredArgs.join(', ')}; passed: ${call.passedArgs.join(', ') || '(none)'})`
        );
      }
    }
    expect(violations, `Invoke calls missing required args:\n${violations.join('\n')}`).toEqual([]);
  });
});
