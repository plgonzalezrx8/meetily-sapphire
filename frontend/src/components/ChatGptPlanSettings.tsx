import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Button } from '@/components/ui/button';
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import { Label } from '@/components/ui/label';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select';
import { Switch } from '@/components/ui/switch';

interface ChatGptStatus {
  connected: boolean;
  email: string | null;
  autoSummary: boolean;
  planEnabled: boolean;
  revocationConfirmed: boolean | null;
  accountKey: string | null;
  welcomeKey: string | null;
  registeredAccounts: { accountKey: string; email: string | null }[];
  disconnectPending: boolean;
  connecting: boolean;
}

interface ChatGptModel {
  slug: string;
  displayName: string;
}

interface ChatGptPlanSettingsProps {
  model: string;
  onModelChange: (model: string) => void;
  onReadyChange: (ready: boolean) => void;
}

function messageForError(error: unknown): string {
  const code = String(error).toLowerCase();
  if (code.includes('cancel')) return 'ChatGPT sign-in was canceled.';
  if (code.includes('usage_limit') || code.includes('usage limit')) return 'Your ChatGPT plan usage limit was reached. Manage usage or try again later.';
  if (code.includes('429') || code.includes('rate limit')) return 'ChatGPT is receiving too many requests. Try again later or manage usage.';
  if (code.includes('not_eligible') || code.includes('not eligible') || code.includes('entitlement')) return 'This ChatGPT account is not eligible for plan access. Check your plan or use an API key.';
  if (code.includes('secure') || code.includes('credential store') || code.includes('storage')) return 'Secure ChatGPT session storage is unavailable. Check your system credential store and local app data.';
  if (code.includes('authentication') || code.includes('reauthoriz') || code.includes('expired') || code.includes('revoked') || code.includes('invalid_grant') || code.includes('invalid_token') || code.includes('401')) return 'Your ChatGPT session has expired or needs authorization. Sign in again.';
  if (code.includes('network') || code.includes('connection') || code.includes('reached') || code.includes('timeout') || code.includes('temporar')) return 'ChatGPT could not be reached. Check your connection and try again.';
  return 'ChatGPT could not complete this action. Please try again.';
}

export function ChatGptPlanSettings({ model, onModelChange, onReadyChange }: ChatGptPlanSettingsProps) {
  const [status, setStatus] = useState<ChatGptStatus | null>(null);
  const [models, setModels] = useState<ChatGptModel[]>([]);
  const [loadingModels, setLoadingModels] = useState(false);
  const [modelsError, setModelsError] = useState('');
  const [error, setError] = useState('');
  const [message, setMessage] = useState('');
  const [showWelcome, setShowWelcome] = useState(false);
  const [selectedAccount, setSelectedAccount] = useState('');
  const [pending, setPending] = useState(false);
  const [connecting, setConnecting] = useState(false);
  const [modelsVersion, setModelsVersion] = useState(0);
  const [statusVersion, setStatusVersion] = useState(0);
  const connectAttempt = useRef(0);
  const ownsConnect = useRef(false);
  const connectPromise = useRef<Promise<ChatGptStatus> | null>(null);
  const mounted = useRef(true);

  useEffect(() => {
    let current = true;
    mounted.current = true;
    invoke<ChatGptStatus>('chatgpt_status').then((value) => {
      if (current) {
        setStatus(value);
        setError('');
        setConnecting(value.connecting);
        if (value.connected && value.planEnabled && !value.disconnectPending) showFirstWelcome(value);
      }
    }).catch((err) => {
      if (current) setError(messageForError(err));
    });
    return () => {
      current = false;
      mounted.current = false;
      ++connectAttempt.current;
      if (ownsConnect.current) {
        ownsConnect.current = false;
        void invoke('chatgpt_cancel_connect').catch(() => {});
      }
    };
  }, [statusVersion]);

  useEffect(() => {
    onReadyChange(!!status?.connected && !!status.planEnabled && !status.disconnectPending && !connecting && !pending);
  }, [status?.connected, status?.planEnabled, status?.disconnectPending, connecting, pending, onReadyChange]);

  useEffect(() => {
    const accounts = status?.registeredAccounts || [];
    setSelectedAccount((current) => accounts.some((account) => account.accountKey === current)
      ? current : status?.accountKey || accounts[0]?.accountKey || '');
  }, [status]);

  useEffect(() => {
    let current = true;
    setModels([]);
    setModelsError('');
    if (!status?.connected || !status.planEnabled || status.disconnectPending) {
      setLoadingModels(false);
      return () => { current = false; };
    }
    setLoadingModels(true);
    invoke<ChatGptModel[]>('chatgpt_models').then((value) => {
      if (current) setModels(value);
    }).catch((err) => {
      if (current) setModelsError(messageForError(err));
    }).finally(() => {
      if (current) setLoadingModels(false);
    });
    return () => { current = false; };
  }, [status?.connected, status?.email, status?.planEnabled, status?.disconnectPending, modelsVersion]);

  function showFirstWelcome(result: ChatGptStatus) {
    const welcomeKey = `chatgptPlanWelcomeShown:${result.welcomeKey || result.accountKey || 'device'}`;
    if (!localStorage.getItem(welcomeKey)) {
      localStorage.setItem(welcomeKey, 'true');
      setShowWelcome(true);
    }
  }

  const connect = async (useDifferentAccount = false, accountKey?: string, reconsent = false) => {
    const attempt = ++connectAttempt.current;
    ownsConnect.current = true;
    setConnecting(true);
    setError('');
    setMessage('');
    const request = invoke<ChatGptStatus>('chatgpt_connect', {
      useDifferentAccount,
      ...(accountKey ? { accountKey } : {}),
      ...(reconsent ? { reconsent } : {}),
    });
    connectPromise.current = request;
    try {
      const result = await request;
      if (!mounted.current || connectAttempt.current !== attempt) return;
      setStatus(result);
      setSelectedAccount(result.accountKey || '');
      setModelsVersion((version) => version + 1);
      if (result.connected && result.planEnabled) {
        showFirstWelcome(result);
        if (result.revocationConfirmed === false) {
          setMessage('The previous ChatGPT session may still be connected. Check your ChatGPT settings to remove it.');
        }
      } else {
        setMessage('Connected, but this account has not granted ChatGPT plan access. Reconnect to grant access.');
      }
    } catch (err) {
      if (mounted.current && connectAttempt.current === attempt) setError(messageForError(err));
    } finally {
      if (connectPromise.current === request) {
        connectPromise.current = null;
        ownsConnect.current = false;
      }
      if (mounted.current && connectAttempt.current === attempt) {
        setConnecting(false);
      }
    }
  };

  const cancelConnect = async () => {
    ++connectAttempt.current;
    setPending(true);
    try {
      await invoke('chatgpt_cancel_connect');
      await connectPromise.current?.catch(() => {});
      const result = await invoke<ChatGptStatus>('chatgpt_status');
      if (mounted.current) {
        setConnecting(result.connecting);
        setStatus(result);
        if (result.connected && result.planEnabled && (!status?.connected || !status.planEnabled || result.accountKey !== status.accountKey)) {
          setSelectedAccount(result.accountKey || '');
          setModelsVersion((version) => version + 1);
          showFirstWelcome(result);
          setMessage(result.revocationConfirmed === false
            ? 'Sign-in finished before cancellation. The previous session may still be connected; check ChatGPT settings to remove it.'
            : 'Sign-in finished before cancellation.');
        } else {
          setMessage('Sign-in canceled.');
        }
      }
    } catch (err) {
      if (mounted.current) setError(messageForError(err));
    } finally {
      if (mounted.current) setPending(false);
    }
  };

  const disconnect = async () => {
    setPending(true);
    setError('');
    setMessage('');
    try {
      const result = await invoke<ChatGptStatus>('chatgpt_disconnect');
      if (!mounted.current) return;
      setStatus(result);
      setMessage(result.disconnectPending ? 'Disconnect is incomplete. Retry to finish disconnecting.' : result.revocationConfirmed === false
        ? 'Disconnected on this device. OpenAI could not confirm revocation; remove the connection in ChatGPT settings.'
        : 'ChatGPT disconnected.');
    } catch (err) {
      if (mounted.current) setError(messageForError(err));
    } finally {
      if (mounted.current) setPending(false);
    }
  };

  const setAutoSummary = async (enabled: boolean) => {
    setPending(true);
    setError('');
    try {
      const result = await invoke<ChatGptStatus>('chatgpt_set_auto_summary', { enabled });
      if (mounted.current) setStatus(result);
    } catch (err) {
      if (mounted.current) setError(messageForError(err));
    } finally {
      if (mounted.current) setPending(false);
    }
  };

  const openUsage = async () => {
    try {
      await invoke('open_external_url', { url: 'https://chatgpt.com/settings/usage' });
    } catch {
      setError('Could not open usage settings. Visit https://chatgpt.com/settings/usage in your browser.');
    }
  };

  return (
    <div className="space-y-3 border-t pt-4">
      <p className="text-sm font-medium">Using ChatGPT plan</p>
      <p className="text-sm text-muted-foreground">
        Selected transcript text and summary instructions go directly to OpenAI under your connected ChatGPT plan.
        Audio stays on this device. Access and refresh credentials are protected on this computer.
        Automatic summaries require a separate opt-in, which you can turn off at any time.
      </p>
      {status?.disconnectPending && <p role="alert" className="text-sm">Disconnect was interrupted. Finish disconnecting before signing in again.</p>}
      {status?.connected && !status.disconnectPending && (
        <p className="text-sm">Connected{status.email ? ` as ${status.email}` : ''}
          {!status.planEnabled && ' — ChatGPT plan access has not been granted.'}
        </p>
      )}
      {!status?.disconnectPending && (status?.registeredAccounts?.length || 0) > 1 && (
        <div className="space-y-1">
          <Label htmlFor="chatgpt-account">Saved ChatGPT account</Label>
          <Select value={selectedAccount} onValueChange={setSelectedAccount} disabled={pending || connecting}>
            <SelectTrigger id="chatgpt-account"><SelectValue placeholder="Select an account" /></SelectTrigger>
            <SelectContent>
              {status!.registeredAccounts.map((account, index) => (
                <SelectItem key={account.accountKey} value={account.accountKey}>{account.email ? `${account.email} (saved account ${index + 1})` : `Saved account ${index + 1}`}</SelectItem>
              ))}
            </SelectContent>
          </Select>
          {status?.connected && selectedAccount && selectedAccount !== status.accountKey && (
            <Button type="button" variant="outline" onClick={() => connect(false, selectedAccount)} disabled={pending || connecting}>Use selected account</Button>
          )}
        </div>
      )}
      {status?.connected && status.planEnabled && !status.disconnectPending && (
        <div className="space-y-1">
          <Label htmlFor="chatgpt-model">ChatGPT plan model</Label>
          <Select value={model} onValueChange={onModelChange} disabled={loadingModels || models.length === 0}>
            <SelectTrigger id="chatgpt-model"><SelectValue placeholder={loadingModels ? 'Loading models...' : 'Select a model'} /></SelectTrigger>
            <SelectContent>
              {model && !models.some((item) => item.slug === model) && <SelectItem value={model}>{model} (saved)</SelectItem>}
              {models.map((item) => <SelectItem key={item.slug} value={item.slug}>{item.displayName}</SelectItem>)}
            </SelectContent>
          </Select>
          <p className="text-xs text-muted-foreground">Models available to your connected ChatGPT account.</p>
          {!loadingModels && !modelsError && models.length === 0 && (
            <p className="text-sm">No models were returned for this account.</p>
          )}
        </div>
      )}
      {modelsError && <p role="alert" className="text-sm text-red-600">{modelsError}</p>}
      {status?.connected && status.planEnabled && !status.disconnectPending && (
        <div className="flex items-center justify-between gap-4">
          <Label htmlFor="chatgpt-auto-summary">Allow automatic summaries using my ChatGPT plan</Label>
          <Switch id="chatgpt-auto-summary" checked={status.autoSummary} onCheckedChange={setAutoSummary} disabled={pending} />
        </div>
      )}
      {error && <p role="alert" className="text-sm text-red-600">{error}</p>}
      {message && <p role="status" className="text-sm">{message}</p>}
      <div className="flex flex-wrap gap-2">
        {connecting ? (
          <Button type="button" variant="outline" onClick={cancelConnect} disabled={pending}>Cancel sign-in</Button>
        ) : status && !status.disconnectPending && (!status.connected || !status.planEnabled) ? (
          <Button type="button" onClick={() => connect(false, status.connected ? status.accountKey || undefined : selectedAccount || undefined, status.connected && !status.planEnabled)} disabled={pending} data-oai-siwc-button="" className="bg-black text-white hover:bg-black/90">
            Continue with ChatGPT
          </Button>
        ) : null}
        {!!status?.registeredAccounts?.length && !status.disconnectPending && !connecting && <Button type="button" variant="outline" onClick={() => connect(true)} disabled={pending}>Use a different account</Button>}
        {(status?.connected || status?.disconnectPending) && <Button type="button" variant="outline" onClick={disconnect} disabled={pending || connecting}>{status.disconnectPending ? 'Finish disconnecting' : 'Disconnect'}</Button>}
        {!status && error && <Button type="button" variant="outline" onClick={() => setStatusVersion((version) => version + 1)}>Retry connection status</Button>}
        <Button type="button" variant={(error || modelsError).includes('usage limit') || (error || modelsError).includes('too many requests') ? 'default' : 'link'} onClick={openUsage}>Manage usage</Button>
      </div>
      {(modelsError || (!loadingModels && models.length === 0)) && status?.connected && status.planEnabled && !status.disconnectPending && (
        <Button type="button" variant="outline" size="sm" onClick={() => setModelsVersion((version) => version + 1)}>Retry models</Button>
      )}
      <Dialog open={showWelcome} onOpenChange={setShowWelcome}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>You’re using your ChatGPT plan</DialogTitle>
            <DialogDescription>
              Eligible summaries in Meetily Sapphire use your ChatGPT plan. You can manage usage in ChatGPT settings.
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button type="button" variant="outline" onClick={openUsage}>Manage usage</Button>
            <Button type="button" onClick={() => setShowWelcome(false)}>Got it</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
