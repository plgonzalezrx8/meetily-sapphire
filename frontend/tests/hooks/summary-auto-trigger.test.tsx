import { afterAll, afterEach, beforeEach, expect, mock, test } from 'bun:test';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';
import { renderToStaticMarkup } from 'react-dom/server';

const originalCore = { ...await import('@tauri-apps/api/core') };
const originalEvent = { ...await import('@tauri-apps/api/event') };
const originalConfig = { ...await import('../../src/contexts/ConfigContext') };
const originalSidebar = { ...await import('../../src/components/Sidebar/SidebarProvider') };
const originalAnalytics = { ...await import('../../src/lib/analytics') };
const originalMeetingData = { ...await import('../../src/hooks/meeting-details/useMeetingData') };
const originalGeneration = { ...await import('../../src/hooks/meeting-details/useSummaryGeneration') };
const originalTemplates = { ...await import('../../src/hooks/meeting-details/useTemplates') };
const originalCopy = { ...await import('../../src/hooks/meeting-details/useCopyOperations') };
const originalOperations = { ...await import('../../src/hooks/meeting-details/useMeetingOperations') };
const originalTranscriptPanel = { ...await import('../../src/components/MeetingDetails/TranscriptPanel') };
const originalSummaryPanel = { ...await import('../../src/components/MeetingDetails/SummaryPanel') };
const originalSplit = { ...await import('../../src/components/MeetingDetails/MeetingDetailsSplitView') };

afterAll(() => {
  mock.module('@tauri-apps/api/core', () => originalCore);
  mock.module('@tauri-apps/api/event', () => originalEvent);
  mock.module('../../src/contexts/ConfigContext', () => originalConfig);
  mock.module('../../src/components/Sidebar/SidebarProvider', () => originalSidebar);
  mock.module('../../src/lib/analytics', () => originalAnalytics);
  mock.module('../../src/hooks/meeting-details/useMeetingData', () => originalMeetingData);
  mock.module('../../src/hooks/meeting-details/useSummaryGeneration', () => originalGeneration);
  mock.module('../../src/hooks/meeting-details/useTemplates', () => originalTemplates);
  mock.module('../../src/hooks/meeting-details/useCopyOperations', () => originalCopy);
  mock.module('../../src/hooks/meeting-details/useMeetingOperations', () => originalOperations);
  mock.module('../../src/components/MeetingDetails/TranscriptPanel', () => originalTranscriptPanel);
  mock.module('../../src/components/MeetingDetails/SummaryPanel', () => originalSummaryPanel);
  mock.module('../../src/components/MeetingDetails/MeetingDetailsSplitView', () => originalSplit);
});

const invoke = mock(async (_command: string, _args?: Record<string, unknown>) => ({}));
const emit = mock(async (_event: string, _payload?: unknown) => {});
const generate = mock(async (_prompt: string, _origin: string) => {});
const autoComplete = mock(() => {});
let autoEnabled = true;
let summaryProps: any;
mock.module('@tauri-apps/api/core', () => ({ ...originalCore, invoke }));
mock.module('@tauri-apps/api/event', () => ({ ...originalEvent, emit }));
mock.module('../../src/contexts/ConfigContext', () => ({ useConfig: () => ({
  modelConfig: { provider: 'openai-chatgpt', model: 'synthetic', whisperModel: 'base' },
  setModelConfig() {}, isModelConfigLoading: false, isAutoSummary: autoEnabled,
}) }));
mock.module('../../src/components/Sidebar/SidebarProvider', () => ({ useSidebar: () => ({}) }));
mock.module('../../src/lib/analytics', () => ({ default: { trackPageView() {} } }));
mock.module('../../src/hooks/meeting-details/useMeetingData', () => ({ useMeetingData: () => ({
  transcripts: [{ text: 'Synthetic transcript', timestamp: '00:00' }], aiSummary: null,
}) }));
mock.module('../../src/hooks/meeting-details/useSummaryGeneration', () => ({ useSummaryGeneration: () => ({
  summaryStatus: 'idle', handleGenerateSummary: generate,
}) }));
mock.module('../../src/hooks/meeting-details/useTemplates', () => ({ useTemplates: () => ({
  availableTemplates: [], selectedTemplate: 'default',
}) }));
mock.module('../../src/hooks/meeting-details/useCopyOperations', () => ({ useCopyOperations: () => ({}) }));
mock.module('../../src/hooks/meeting-details/useMeetingOperations', () => ({ useMeetingOperations: () => ({}) }));
mock.module('../../src/components/MeetingDetails/TranscriptPanel', () => ({ TranscriptPanel: () => null }));
mock.module('../../src/components/MeetingDetails/SummaryPanel', () => ({ SummaryPanel: (props: any) => {
  summaryProps = props;
  return null;
} }));
mock.module('../../src/components/MeetingDetails/MeetingDetailsSplitView', () => ({ MeetingDetailsSplitView: ({ summary }: any) => summary }));

const { default: PageContent } = await import('../../src/app/meeting-details/page-content');
let renderer: ReactTestRenderer | undefined;
beforeEach(() => {
  autoEnabled = true;
  summaryProps = undefined;
  invoke.mockClear(); emit.mockClear(); generate.mockClear(); autoComplete.mockClear();
});
afterEach(async () => { if (renderer) await act(async () => renderer!.unmount()); renderer = undefined; });

async function show(shouldAutoGenerate: boolean) {
  await act(async () => {
    renderer = create(<PageContent meeting={{ id: 'synthetic-meeting' }} summaryData={null}
      initialSummary={null} shouldAutoGenerate={shouldAutoGenerate} onAutoGenerateComplete={autoComplete} />);
  });
}

test('meeting route tags a post-recording summary as automatic and respects the global switch', async () => {
  await show(true);
  expect(generate).toHaveBeenCalledWith('', 'automatic');
  expect(autoComplete).toHaveBeenCalledTimes(1);
  await act(async () => renderer!.unmount());
  renderer = undefined;
  generate.mockClear();
  autoEnabled = false;
  await show(true);
  expect(generate).not.toHaveBeenCalled();
});

test('meeting settings never save or broadcast stale API keys for ChatGPT plan', async () => {
  await show(false);
  await act(async () => summaryProps.onSaveModelConfig({
    provider: 'openai-chatgpt', model: 'synthetic', whisperModel: 'base',
    apiKey: 'stale-api-key', customOpenAIApiKey: 'stale-custom-key',
  }));
  expect(invoke.mock.calls[0]?.[1]).toMatchObject({ provider: 'openai-chatgpt', apiKey: null });
  expect(emit.mock.calls[0]?.[1]).toMatchObject({ apiKey: null, customOpenAIApiKey: null });
});

test('the meeting panel identifies ChatGPT plan use and exposes usage management', () => {
  const props: any = {
    meeting: { id: 'synthetic-meeting', title: 'Meeting', created_at: '2026-01-01' },
    meetingTitle: 'Meeting', isSummaryDirty: false, summaryRef: { current: null }, isSaving: false,
    onSaveAll: async () => {}, onCopySummary: async () => {}, aiSummary: null,
    summaryStatus: 'idle', transcripts: [],
    modelConfig: { provider: 'openai-chatgpt', model: 'synthetic', whisperModel: 'base' },
    setModelConfig() {}, onSaveModelConfig: async () => {}, onGenerateSummary: async () => {},
    onStopGeneration() {}, customPrompt: '', onSaveSummary: async () => {}, onSummaryChange() {},
    onDirtyChange() {}, summaryError: null, onRegenerateSummary: async () => {},
    getSummaryStatusMessage: () => '', availableTemplates: [], selectedTemplate: '', onTemplateSelect() {},
  };
  const planHtml = renderToStaticMarkup(<originalSummaryPanel.SummaryPanel {...props} />);
  expect(planHtml).toContain('Uses your ChatGPT plan');
  expect(planHtml).toContain('Manage usage');
  const apiHtml = renderToStaticMarkup(<originalSummaryPanel.SummaryPanel {...props}
    modelConfig={{ ...props.modelConfig, provider: 'openai' }} />);
  expect(apiHtml).not.toContain('Uses your ChatGPT plan');
});
