import { afterAll, afterEach, beforeEach, expect, mock, test } from 'bun:test';
import { createElement, useState } from 'react';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';
import type { ModelConfig } from '../../src/components/ModelSettingsModal';

const modulePaths = [
  '@tauri-apps/api/core', '../../src/contexts/ConfigContext', '../../src/contexts/OllamaDownloadContext',
  '../../src/components/Sidebar/SidebarProvider', '../../src/components/BuiltInModelManager',
  '../../src/components/ui/button', '../../src/components/ui/input', '../../src/components/ui/label',
  '../../src/components/ui/select', '../../src/components/ui/popover', '../../src/components/ui/command',
  '../../src/components/ui/alert', '../../src/components/ui/scroll-area', '../../src/components/ui/switch',
  '../../src/components/ui/dialog', 'sonner',
];
const originals = await Promise.all(modulePaths.map(async (path) => [path, { ...await import(path) }] as const));
afterAll(() => { for (const [path, exports] of originals) mock.module(path, () => exports); });

let context: any;
let handlers: Record<string, (args?: any) => Promise<any>>;
const invoke = mock((command: string, args?: any) => {
  if (handlers[command]) return handlers[command](args);
  if (command === 'api_get_auto_generate_setting') return Promise.resolve(false);
  if (command === 'get_groq_models') return Promise.resolve([]);
  throw new Error(`Unexpected command ${command}`);
});
mock.module('@tauri-apps/api/core', () => ({ invoke }));
mock.module('../../src/contexts/ConfigContext', () => ({ useConfig: () => context }));
mock.module('../../src/contexts/OllamaDownloadContext', () => ({ useOllamaDownload: () => ({
  isDownloading: () => false, getProgress: () => undefined, downloadingModels: new Set(),
}) }));
mock.module('../../src/components/Sidebar/SidebarProvider', () => ({ useSidebar: () => ({ serverAddress: '' }) }));
mock.module('../../src/components/BuiltInModelManager', () => ({ BuiltInModelManager: () => null }));
mock.module('../../src/components/ui/button', () => ({ Button: ({ children, ...props }: any) => <button {...props}>{children}</button> }));
mock.module('../../src/components/ui/input', () => ({ Input: (props: any) => <input {...props} /> }));
mock.module('../../src/components/ui/label', () => ({ Label: ({ children, ...props }: any) => <label {...props}>{children}</label> }));
mock.module('../../src/components/ui/select', () => ({
  Select: ({ children, ...props }: any) => createElement('test-select', props, children),
  SelectContent: ({ children }: any) => <div>{children}</div>,
  SelectItem: ({ children, ...props }: any) => createElement('test-select-item', props, children),
  SelectTrigger: ({ children, ...props }: any) => <div {...props}>{children}</div>,
  SelectValue: () => null,
}));
mock.module('../../src/components/ui/popover', () => ({
  Popover: ({ children }: any) => <div>{children}</div>, PopoverTrigger: ({ children }: any) => <div>{children}</div>,
  PopoverContent: ({ children }: any) => <div>{children}</div>,
}));
mock.module('../../src/components/ui/command', () => ({
  Command: ({ children }: any) => <div>{children}</div>, CommandEmpty: ({ children }: any) => <div>{children}</div>,
  CommandGroup: ({ children }: any) => <div>{children}</div>, CommandInput: () => null,
  CommandItem: ({ children, ...props }: any) => createElement('test-command-item', props, children),
  CommandList: ({ children }: any) => <div>{children}</div>,
}));
mock.module('../../src/components/ui/alert', () => ({
  Alert: ({ children }: any) => <div>{children}</div>, AlertDescription: ({ children }: any) => <div>{children}</div>,
}));
mock.module('../../src/components/ui/scroll-area', () => ({ ScrollArea: ({ children }: any) => <div>{children}</div> }));
mock.module('../../src/components/ui/switch', () => ({ Switch: (props: any) => createElement('test-switch', props) }));
mock.module('../../src/components/ui/dialog', () => ({
  Dialog: ({ children, open }: any) => open ? <div>{children}</div> : null,
  DialogContent: ({ children }: any) => <div>{children}</div>, DialogDescription: ({ children }: any) => <div>{children}</div>,
  DialogFooter: ({ children }: any) => <div>{children}</div>, DialogHeader: ({ children }: any) => <div>{children}</div>,
  DialogTitle: ({ children }: any) => <div>{children}</div>,
}));
mock.module('sonner', () => ({ toast: { info() {}, error() {}, success() {}, warning() {} } }));

const { ModelSettingsModal } = await import('../../src/components/ModelSettingsModal');
let renderer: ReactTestRenderer;
let current: ModelConfig;
const onSave = mock((_config: ModelConfig) => {});
const storage = new Map<string, string>();
const originalStorage = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
type Status = { connected: boolean; email: string | null; autoSummary: boolean; planEnabled: boolean; revocationConfirmed: boolean | null; accountKey: string | null; welcomeKey: string | null; disconnectPending: boolean; connecting: boolean; registeredAccounts: { accountKey: string; email: string | null }[] };
const disconnected: Status = { connected: false, email: null, autoSummary: false, planEnabled: false, revocationConfirmed: null, accountKey: null, welcomeKey: null, disconnectPending: false, connecting: false, registeredAccounts: [] };
const connected: Status = { connected: true, email: 'person@example.test', autoSummary: false, planEnabled: true, revocationConfirmed: null, accountKey: 'account-a', welcomeKey: 'user-a', disconnectPending: false, connecting: false, registeredAccounts: [{ accountKey: 'account-a', email: 'person@example.test' }] };

function Harness({ initial }: { initial: ModelConfig }) {
  const [config, setConfig] = useState(initial);
  current = config;
  context = {
    modelConfig: config, setModelConfig: setConfig,
    providerApiKeys: { openai: 'openai-test-key', claude: 'claude-test-key', groq: null, openrouter: null },
    updateProviderApiKey: () => {},
  };
  return <ModelSettingsModal modelConfig={config} setModelConfig={setConfig} onSave={onSave} skipInitialFetch />;
}

const initial: ModelConfig = { provider: 'openai', model: 'saved-exact-id', whisperModel: 'base', apiKey: 'openai-test-key', customOpenAIApiKey: 'custom-test-key' };
const text = () => JSON.stringify(renderer.toJSON());
const selectProvider = async (provider: string) => {
  await act(async () => renderer.root.findAllByType('test-select' as any)[0].props.onValueChange(provider));
};
const click = async (label: string) => {
  const button = renderer.root.findAllByType('button').find((item) => item.children.includes(label));
  if (!button) throw new Error(`Button not found: ${label}`);
  await act(async () => { await button.props.onClick(); });
};
const deferred = <T,>() => {
  let resolve!: (value: T) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<T>((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
};
beforeEach(() => {
  storage.clear(); onSave.mockClear(); invoke.mockClear();
  Object.defineProperty(globalThis, 'localStorage', { configurable: true, value: {
    getItem: (key: string) => storage.get(key) ?? null,
    setItem: (key: string, value: string) => storage.set(key, value),
  } });
  handlers = {
    get_openai_models: async () => [{ id: 'api-model' }],
    get_anthropic_models: async () => [{ id: 'claude-model' }],
    chatgpt_status: async () => disconnected,
  };
});
afterEach(async () => {
  if (renderer) await act(async () => renderer.unmount());
  renderer = undefined as unknown as ReactTestRenderer;
  if (originalStorage) Object.defineProperty(globalThis, 'localStorage', originalStorage);
  else Reflect.deleteProperty(globalThis, 'localStorage');
});
async function show() { await act(async () => { renderer = create(<Harness initial={initial} />); }); }

test('preserves exact saved IDs through provider switches and ignores obsolete discovery responses', async () => {
  const first = deferred<{ id: string }[]>();
  const second = deferred<{ id: string }[]>();
  let calls = 0;
  handlers.get_openai_models = () => (++calls === 1 ? first.promise : second.promise);
  await show();
  expect(current.model).toBe('saved-exact-id');
  await selectProvider('claude');
  await selectProvider('openai');
  expect(current.model).toBe('saved-exact-id');
  await act(async () => second.resolve([{ id: 'fresh-id' }]));
  await act(async () => first.resolve([{ id: 'obsolete-id' }]));
  expect(text()).toContain('fresh-id');
  expect(text()).not.toContain('obsolete-id');
  expect(text()).toContain('returned for this API key');
  expect(current.model).toBe('saved-exact-id');
  expect(text()).not.toContain('Case-Sensitive-Exact-ID');
  await act(async () => renderer.root.findByProps({ id: 'hosted-model-id' }).props.onChange({ target: { value: 'Case-Sensitive-Exact-ID' } }));
  await click('Save');
  expect(onSave.mock.calls[0][0].model).toBe('Case-Sensitive-Exact-ID');
});

test('labels discovery failures as unverified suggestions and accepts an exact ID', async () => {
  handlers.get_openai_models = async () => { throw new Error('synthetic authentication error'); };
  await show();
  expect(text()).toContain('Could not verify models');
  expect(text()).toContain('unverified suggestions');
  expect(current.model).toBe('saved-exact-id');
});

test('plan flow exposes privacy, cancel, explicit automatic consent, disconnect, and first sign-in welcome', async () => {
  const pendingConnect = deferred<typeof connected>();
  let status: Status = disconnected;
  let connects = 0;
  handlers.chatgpt_status = async () => status;
  handlers.chatgpt_connect = () => ++connects === 1 ? pendingConnect.promise : Promise.resolve(status = connected);
  handlers.chatgpt_cancel_connect = async () => { pendingConnect.reject(new Error('canceled')); };
  handlers.chatgpt_models = async () => [{ slug: 'plan-model', displayName: 'Plan Model' }];
  handlers.chatgpt_set_auto_summary = async ({ enabled }) => status = { ...connected, autoSummary: enabled };
  handlers.chatgpt_disconnect = async () => status = { ...disconnected, revocationConfirmed: false };
  await show();
  await selectProvider('openai-chatgpt');
  expect(text()).toContain('Selected transcript text and summary instructions go directly to OpenAI');
  expect(text()).toContain('Audio stays on this device');
  expect(current.apiKey).toBeNull();
  expect(current.customOpenAIApiKey).toBeNull();
  await act(async () => { renderer.root.findAllByType('button').find((item) => item.children.includes('Continue with ChatGPT'))!.props.onClick(); });
  await click('Cancel sign-in');
  expect(text()).toContain('Sign-in canceled');
  expect(text()).not.toContain('person@example.test');
  await click('Continue with ChatGPT');
  expect(text()).toContain('You’re using your ChatGPT plan');
  expect(text()).toContain('Plan Model');
  expect(invoke.mock.calls.some(([command]) => command === 'chatgpt_set_auto_summary')).toBe(false);
  await click('Got it');
  await act(async () => renderer.root.findByType('test-switch' as any).props.onCheckedChange(true));
  await act(async () => renderer.root.findByType('test-switch' as any).props.onCheckedChange(false));
  expect(invoke.mock.calls.filter(([command]) => command === 'chatgpt_set_auto_summary').map(([, args]) => args.enabled)).toEqual([true, false]);
  await act(async () => renderer.root.findAllByType('test-select' as any)[1].props.onValueChange('plan-model'));
  await click('Save');
  expect(onSave.mock.calls[0][0]).toMatchObject({ provider: 'openai-chatgpt', model: 'plan-model', apiKey: null, customOpenAIApiKey: null });
  await click('Disconnect');
  expect(text()).toContain('OpenAI could not confirm revocation');
  await click('Continue with ChatGPT');
  expect(text()).not.toContain('You’re using your ChatGPT plan');
});

test('leaving settings cancels an active sign-in and reopening can cancel a recovered one', async () => {
  const pendingConnect = deferred<Status>();
  let status: Status = disconnected;
  handlers.chatgpt_status = async () => status;
  handlers.chatgpt_connect = () => pendingConnect.promise;
  handlers.chatgpt_cancel_connect = async () => undefined;
  await show();
  await selectProvider('openai-chatgpt');
  await act(async () => { renderer.root.findAllByType('button').find((item) => item.children.includes('Continue with ChatGPT'))!.props.onClick(); });
  await selectProvider('openai');
  expect(invoke.mock.calls.filter(([command]) => command === 'chatgpt_cancel_connect')).toHaveLength(1);
  await act(async () => pendingConnect.resolve(connected));
  expect(current.provider).toBe('openai');
  status = { ...disconnected, connecting: true };
  await selectProvider('openai-chatgpt');
  expect(text()).toContain('Cancel sign-in');
  handlers.chatgpt_cancel_connect = async () => { status = disconnected; };
  await click('Cancel sign-in');
  expect(text()).toContain('Sign-in canceled');
  expect(text()).toContain('Continue with ChatGPT');
});

test('confirms a sign-in that completed just before cancellation', async () => {
  const pendingConnect = deferred<Status>();
  let status: Status = disconnected;
  handlers.chatgpt_status = async () => status;
  handlers.chatgpt_connect = () => pendingConnect.promise;
  handlers.chatgpt_models = async () => [{ slug: 'plan-model', displayName: 'Plan Model' }];
  handlers.chatgpt_cancel_connect = async () => { status = connected; pendingConnect.resolve(connected); };
  await show();
  await selectProvider('openai-chatgpt');
  await act(async () => { renderer.root.findAllByType('button').find((item) => item.children.includes('Continue with ChatGPT'))!.props.onClick(); });
  await click('Cancel sign-in');
  expect(text()).toContain('Sign-in finished before cancellation');
  expect(text()).toContain('You’re using your ChatGPT plan');
});

test('shows a missed first welcome after reopening and does not repeat it', async () => {
  handlers.chatgpt_status = async () => connected;
  handlers.chatgpt_models = async () => [{ slug: 'plan-model', displayName: 'Plan Model' }];
  await show();
  await selectProvider('openai-chatgpt');
  expect(text()).toContain('You’re using your ChatGPT plan');
  await click('Got it');
  await selectProvider('openai');
  await selectProvider('openai-chatgpt');
  expect(text()).not.toContain('You’re using your ChatGPT plan');
});

test.each([
  ['subscription_sharing_usage_limit_exceeded', 'usage limit was reached'],
  ['HTTP 429', 'too many requests'],
  ['session expired or was revoked', 'Sign in again'],
  ['could not be reached', 'Check your connection'],
  ['Secure ChatGPT session storage is unavailable', 'system credential store'],
  ['subscription_sharing_user_not_eligible', 'not eligible'],
])('provides actionable, sanitized guidance for %s', async (failure, expected) => {
  handlers.chatgpt_status = async () => { throw new Error(failure); };
  await show();
  await selectProvider('openai-chatgpt');
  expect(text()).toContain(expected);
  expect(text()).toContain('Retry connection status');
});

test('switches to a saved account and offers a different account after disconnect', async () => {
  const accounts = [
    { accountKey: 'account-a', email: 'person@example.test' },
    { accountKey: 'account-b', email: 'other@example.test' },
  ];
  let status: Status = { ...connected, registeredAccounts: accounts };
  handlers.chatgpt_status = async () => status;
  handlers.chatgpt_models = async () => [{ slug: 'plan-model', displayName: 'Plan Model' }];
  handlers.chatgpt_connect = async ({ accountKey }) => status = {
    ...connected, accountKey, email: 'other@example.test', registeredAccounts: accounts, revocationConfirmed: false,
  };
  handlers.chatgpt_disconnect = async () => status = { ...disconnected, registeredAccounts: accounts, revocationConfirmed: true };
  await show();
  await selectProvider('openai-chatgpt');
  expect(text()).toContain('Saved ChatGPT account');
  await act(async () => renderer.root.findAllByType('test-select' as any)[1].props.onValueChange('account-b'));
  await click('Use selected account');
  expect(invoke.mock.calls.find(([command]) => command === 'chatgpt_connect')?.[1]).toEqual({ useDifferentAccount: false, accountKey: 'account-b' });
  await click('Got it');
  expect(text()).toContain('previous ChatGPT session may still be connected');
  await click('Disconnect');
  expect(text()).toContain('Use a different account');
  expect(text()).toContain('other@example.test');
});

test('reconsents for an account without plan access and finishes an interrupted disconnect', async () => {
  let status: Status = { ...connected, planEnabled: false };
  handlers.chatgpt_status = async () => status;
  handlers.chatgpt_models = async () => [{ slug: 'plan-model', displayName: 'Plan Model' }];
  handlers.chatgpt_connect = async () => status = connected;
  handlers.chatgpt_disconnect = async () => status = disconnected;
  await show();
  await selectProvider('openai-chatgpt');
  await click('Continue with ChatGPT');
  expect(invoke.mock.calls.find(([command]) => command === 'chatgpt_connect')?.[1]).toEqual({ useDifferentAccount: false, accountKey: 'account-a', reconsent: true });
  await click('Got it');
  await selectProvider('openai');
  status = { ...connected, disconnectPending: true };
  await selectProvider('openai-chatgpt');
  expect(text()).toContain('Disconnect was interrupted');
  expect(text()).not.toContain('Continue with ChatGPT');
  await click('Finish disconnecting');
  expect(text()).toContain('ChatGPT disconnected');
});
