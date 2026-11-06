// External sources (MCP) settings (requirement Q45).
//
// - Register local MCP servers: a program plus arguments, started without a shell.
// - "Check tools" starts a *saved* server once and lists its tools (the native side looks the
//   server up by id in the saved settings, so unsaved edits are never executed).
// - Only read-only tools can be the search tool, and the question goes into one text input
//   the user picks from the tool's schema. Nothing is guessed from names or descriptions.
import { useState } from 'react';
import { Plus, Trash2, Wrench } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import type { Language, McpServer } from '../../lib/types';
import { mcpText, type McpMessage } from './mcp-messages';
import './mcp.css';

/** A tool as the native side reports it. */
interface McpTool {
  name: string;
  description: string;
  inputSchema: { properties?: Record<string, { type?: string }> };
  readOnly: boolean;
  usable: boolean;
  suggestedArgument: string | null;
}

type Settings = { enabled: boolean; servers: McpServer[] };
const errors: McpMessage[] = [
  'start',
  'timedOut',
  'closed',
  'protocol',
  'server',
  'busy',
  'notFound',
];
const maxServers = 8;

/** Text inputs of a tool's schema: the only places a question can go. */
function textInputs(tool: McpTool) {
  return Object.entries(tool.inputSchema?.properties ?? {})
    .filter(([, property]) => property?.type === 'string')
    .map(([name]) => name);
}

function newId() {
  return typeof crypto.randomUUID === 'function'
    ? crypto.randomUUID()
    : `mcp-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;
}

export function McpSettings({
  value,
  saved,
  onChange,
  language,
  native,
}: {
  value: Settings;
  /** The settings as last saved; probing works only for servers saved exactly like this. */
  saved: Settings;
  onChange: (value: Settings) => void;
  language: Language;
  native: boolean;
}) {
  const t = (key: McpMessage) => mcpText(language, key);
  const [tools, setTools] = useState<Record<string, McpTool[]>>({});
  const [probing, setProbing] = useState<string | null>(null);
  const [failures, setFailures] = useState<Record<string, McpMessage>>({});

  const update = (id: string, patch: Partial<McpServer>) =>
    onChange({
      ...value,
      servers: value.servers.map((server) => (server.id === id ? { ...server, ...patch } : server)),
    });

  const probe = async (id: string) => {
    setProbing(id);
    setFailures(({ [id]: _, ...rest }) => rest);
    try {
      const list = await invoke<McpTool[]>('mcp_probe', { serverId: id });
      setTools((current) => ({ ...current, [id]: list }));
    } catch (failure) {
      const code = String(failure) as McpMessage;
      setFailures((current) => ({ ...current, [id]: errors.includes(code) ? code : 'server' }));
    } finally {
      setProbing(null);
    }
  };

  return (
    <section className="mcp-settings" aria-labelledby="mcp-title">
      <h3 id="mcp-title">{t('title')}</h3>
      <p className="setting-footnote">{t('intro')}</p>
      <label className="setting-check">
        <span>{t('enabled')}</span>
        <input
          type="checkbox"
          checked={value.enabled}
          onChange={(event) => onChange({ ...value, enabled: event.target.checked })}
        />
      </label>

      {value.servers.map((server) => {
        const isSaved =
          JSON.stringify(saved.servers.find((item) => item.id === server.id)) ===
          JSON.stringify(server);
        const list = tools[server.id];
        const readOnly = list?.filter((tool) => tool.usable) ?? [];
        const chosen = readOnly.find((tool) => tool.name === server.search?.tool) ?? null;
        return (
          <div key={server.id} className="mcp-server">
            <div className="mcp-row">
              <label>
                <span>{t('name')}</span>
                <input
                  value={server.name}
                  maxLength={80}
                  onChange={(event) => update(server.id, { name: event.target.value })}
                />
              </label>
              <label>
                <span>{t('command')}</span>
                <input
                  value={server.command}
                  maxLength={1024}
                  spellCheck={false}
                  title={t('commandHint')}
                  onChange={(event) => update(server.id, { command: event.target.value })}
                />
              </label>
              <button
                type="button"
                className="text-button"
                onClick={() =>
                  onChange({
                    ...value,
                    servers: value.servers.filter((item) => item.id !== server.id),
                  })
                }
              >
                <Trash2 size={14} aria-hidden="true" />
                {t('remove')}
              </button>
            </div>
            <label className="mcp-args">
              <span>{t('args')}</span>
              <textarea
                rows={2}
                spellCheck={false}
                value={server.args.join('\n')}
                onChange={(event) =>
                  update(server.id, {
                    args: event.target.value.split('\n').slice(0, 32),
                  })
                }
              />
            </label>

            <div className="mcp-tools">
              <button
                type="button"
                className="text-button"
                disabled={!native || !isSaved || probing !== null}
                title={!isSaved ? t('saveFirst') : undefined}
                onClick={() => void probe(server.id)}
              >
                <Wrench size={14} aria-hidden="true" />
                {probing === server.id ? t('probing') : t('probe')}
              </button>
              {!native ? <small>{t('unavailable')}</small> : null}
              {native && !isSaved ? <small>{t('saveFirst')}</small> : null}
              {failures[server.id] ? (
                <small className="mcp-error" role="alert">
                  {t(failures[server.id])}
                </small>
              ) : null}
            </div>

            {list ? (
              <div className="mcp-mapping">
                <ul className="mcp-tool-list">
                  {list.map((tool) => (
                    <li key={tool.name}>
                      <strong>{tool.name}</strong>
                      <span>{tool.usable ? t('readOnly') : t('notReadOnly')}</span>
                    </li>
                  ))}
                </ul>
                {readOnly.length === 0 ? (
                  <p className="setting-footnote">{t('noTools')}</p>
                ) : (
                  <>
                    <label>
                      <span>{t('searchTool')}</span>
                      <select
                        value={server.search?.tool ?? ''}
                        onChange={(event) => {
                          const tool = readOnly.find((item) => item.name === event.target.value);
                          const argument = tool
                            ? (tool.suggestedArgument ?? textInputs(tool)[0] ?? '')
                            : '';
                          update(server.id, {
                            search: tool && argument ? { tool: tool.name, argument } : null,
                          });
                        }}
                      >
                        <option value="">{t('none')}</option>
                        {readOnly.map((tool) => (
                          <option
                            key={tool.name}
                            value={tool.name}
                            disabled={!textInputs(tool).length}
                          >
                            {tool.name}
                          </option>
                        ))}
                      </select>
                    </label>
                    {chosen ? (
                      <label>
                        <span>{t('argument')}</span>
                        <select
                          value={server.search?.argument ?? ''}
                          onChange={(event) =>
                            update(server.id, {
                              search: { tool: chosen.name, argument: event.target.value },
                            })
                          }
                        >
                          {textInputs(chosen).map((name) => (
                            <option key={name} value={name}>
                              {name}
                            </option>
                          ))}
                        </select>
                      </label>
                    ) : null}
                    {chosen && !textInputs(chosen).length ? (
                      <p className="setting-footnote">{t('noText')}</p>
                    ) : null}
                  </>
                )}
              </div>
            ) : null}
          </div>
        );
      })}

      <button
        type="button"
        className="text-button"
        disabled={value.servers.length >= maxServers}
        title={value.servers.length >= maxServers ? t('maxServers') : undefined}
        onClick={() =>
          onChange({
            ...value,
            servers: [
              ...value.servers,
              { id: newId(), name: '', command: '', args: [], allowedTools: [], search: null },
            ],
          })
        }
      >
        <Plus size={14} aria-hidden="true" />
        {t('add')}
      </button>
    </section>
  );
}
