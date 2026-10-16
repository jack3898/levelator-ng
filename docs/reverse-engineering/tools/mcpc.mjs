// Minimal MCP stdio client for the REA server (`rea --mcp`).
// Usage: node mcpc.mjs <path/to/rea> job.json
// job.json: {"list": true} to list tools, and/or {"calls": [{"name": tool, "args": {...}, "out": "file.json"}]}
import { spawn } from 'node:child_process';
import fs from 'node:fs';
const [,, reaBin, jobFile] = process.argv;
const job = JSON.parse(fs.readFileSync(jobFile, 'utf8'));
const p = spawn(reaBin, ['--mcp'], { stdio: ['pipe', 'pipe', 'inherit'], env: process.env });
let buf = ''; const pending = new Map(); let id = 0;
p.stdout.on('data', d => { buf += d; let i; while ((i = buf.indexOf('\n')) >= 0) { const line = buf.slice(0, i); buf = buf.slice(i + 1); if (!line.trim()) continue; const m = JSON.parse(line); if (m.id != null && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); } } });
const rpc = (method, params) => new Promise(r => { const i = ++id; pending.set(i, r); p.stdin.write(JSON.stringify({ jsonrpc: '2.0', id: i, method, params }) + '\n'); });
await rpc('initialize', { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'c', version: '1' } });
p.stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' }) + '\n');
if (job.list) { const r = await rpc('tools/list', {}); for (const t of r.result.tools) console.log(t.name, '::', JSON.stringify(t.inputSchema.properties ? Object.keys(t.inputSchema.properties) : [])); }
for (const c of job.calls || []) {
  const r = await rpc('tools/call', { name: c.name, arguments: c.args });
  const out = c.out; const text = JSON.stringify(r.result ?? r.error);
  if (out) fs.writeFileSync(out, text); else console.log(text.slice(0, c.max || 3000));
}
p.kill();
