// Inert MCP stdio fixture. It reads no user files and only records this test's
// protocol messages. stdout is reserved for JSON-RPC, never diagnostic output.
import { appendFileSync, readFileSync } from 'node:fs';
import { createInterface } from 'node:readline';

const [trace, control] = process.argv.slice(2);
const record = (event) =>
  appendFileSync(trace, `${JSON.stringify({ pid: process.pid, ...event })}\n`);
const reply = (id, result) =>
  process.stdout.write(`${JSON.stringify({ jsonrpc: '2.0', id, result })}\n`);
record({ event: 'start' });
const input = createInterface({ input: process.stdin });
input.on('line', (line) => {
  const message = JSON.parse(line);
  record({ event: 'message', message });
  if (message.method === 'initialize') {
    reply(message.id, {
      protocolVersion: '2025-06-18',
      capabilities: { tools: {} },
      serverInfo: { name: 'Prism native QA fixture', version: '1.0.0' },
    });
  } else if (message.method === 'tools/list') {
    reply(message.id, {
      tools: [
        {
          name: 'search',
          description: 'Return inert evidence',
          inputSchema: {
            type: 'object',
            properties: { query: { type: 'string' } },
            required: ['query'],
          },
          annotations: { readOnlyHint: true },
        },
        {
          name: 'delete',
          description: 'Never usable',
          inputSchema: {
            type: 'object',
            properties: { query: { type: 'string' } },
            required: ['query'],
          },
          annotations: { readOnlyHint: false },
        },
      ],
    });
  } else if (message.method === 'tools/call') {
    const state = JSON.parse(readFileSync(control, 'utf8'));
    if (state.mode === 'stall') return;
    if (message.params.name !== 'search') throw new Error('A non-read-only tool was called');
    reply(message.id, { content: [{ type: 'text', text: state.text }], isError: false });
  }
});
input.on('close', () => process.exit(0));
