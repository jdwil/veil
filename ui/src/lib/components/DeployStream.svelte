<script lang="ts">
  /**
   * DeployStream — WebSocket-based terraform deploy progress.
   *
   * Connects to /api/projects/{slug}/deploy/ws, sends start, renders live output
   * with per-resource progress indicators.
   */

  interface Props {
    slug: string;
    environment: string;
    deploy_type?: string;
    onDone?: (outputs: Record<string, unknown>) => void;
    onError?: (msg: string) => void;
    onCancel?: () => void;
  }

  let { slug, environment, deploy_type = 'infrastructure', onDone, onError, onCancel }: Props = $props();

  type StepStatus = 'pending' | 'running' | 'done' | 'error';
  type ResourceStatus = 'pending' | 'creating' | 'created' | 'updating' | 'updated' | 'destroying' | 'destroyed' | 'waiting' | 'error';

  interface Step {
    id: string;
    label: string;
    status: StepStatus;
    error?: string;
  }

  interface Resource {
    address: string;
    status: ResourceStatus;
    elapsed?: string;
  }

  let steps: Step[] = $state(deploy_type === 'frontend'
    ? [
        { id: 'generate', label: 'Generate', status: 'pending' },
        { id: 'build', label: 'Build', status: 'pending' },
        { id: 'deploy', label: 'Deploy', status: 'pending' },
      ]
    : [
        { id: 'init', label: 'Terraform init', status: 'pending' },
        { id: 'plan', label: 'Terraform plan', status: 'pending' },
        { id: 'apply', label: 'Terraform apply', status: 'pending' },
      ]
  );

  let resources: Resource[] = $state([]);
  let logs: string[] = $state([]);
  let status: 'connecting' | 'running' | 'done' | 'error' = $state('connecting');
  let errorMsg: string = $state('');
  let summary: string = $state('');
  let outputs: Record<string, unknown> = $state({});
  let ws: WebSocket | null = $state(null);
  let totalResources: number = $state(0);
  let completedResources: number = $state(0);

  $effect(() => {
    connect();
    return () => {
      if (ws) ws.close();
    };
  });

  function connect() {
    const proto = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    const url = `${proto}//${window.location.host}/api/projects/${encodeURIComponent(slug)}/deploy/ws`;
    const socket = new WebSocket(url);
    ws = socket;

    socket.onopen = () => {
      status = 'running';
      socket.send(JSON.stringify({ action: 'start', environment, deploy_type }));
    };

    socket.onmessage = (ev) => {
      try {
        const msg = JSON.parse(ev.data);
        handleMessage(msg);
      } catch {}
    };

    socket.onerror = () => {
      status = 'error';
      errorMsg = 'WebSocket connection failed';
      onError?.(errorMsg);
    };

    socket.onclose = () => {
      if (status === 'running') {
        status = 'error';
        errorMsg = 'Connection lost';
        onError?.(errorMsg);
      }
    };
  }

  function handleMessage(msg: any) {
    switch (msg.type) {
      case 'started':
        status = 'running';
        break;

      case 'step_start':
        steps = steps.map(s => s.id === msg.step ? { ...s, status: 'running' } : s);
        break;

      case 'step_done':
        steps = steps.map(s => s.id === msg.step ? { ...s, status: msg.ok ? 'done' : 'error', error: msg.error } : s);
        break;

      case 'resource':
        // From plan phase — count total resources
        totalResources++;
        break;

      case 'progress':
        handleProgress(msg);
        break;

      case 'plan_summary':
      case 'apply_summary':
        summary = msg.line;
        break;

      case 'log':
        // Keep last 50 log lines
        if (msg.line?.trim()) {
          logs = [...logs.slice(-49), msg.line];
        }
        break;

      case 'done':
        status = 'done';
        outputs = msg.outputs || {};
        summary = 'Deploy complete';
        onDone?.(outputs);
        break;

      case 'error':
        status = 'error';
        errorMsg = msg.message || 'Unknown error';
        onError?.(errorMsg);
        break;
    }
  }

  function handleProgress(msg: any) {
    const addr = msg.resource || '';
    const existing = resources.find(r => r.address === addr);
    if (existing) {
      resources = resources.map(r =>
        r.address === addr ? { ...r, status: msg.status, elapsed: msg.elapsed || r.elapsed } : r
      );
    } else {
      resources = [...resources, { address: addr, status: msg.status, elapsed: msg.elapsed }];
    }

    // Count completed
    if (msg.status === 'created' || msg.status === 'updated' || msg.status === 'destroyed') {
      completedResources++;
    }
  }

  function cancel() {
    ws?.close();
    status = 'error';
    errorMsg = 'Cancelled';
    onCancel?.();
  }

  function progressPercent(): number {
    if (totalResources === 0) {
      // Estimate from steps
      const done = steps.filter(s => s.status === 'done').length;
      return Math.round((done / steps.length) * 100);
    }
    // Init + plan = 20%, apply = 80% weighted by resources
    const stepDone = steps.filter(s => s.status === 'done').length;
    if (stepDone < 2) return Math.round((stepDone / steps.length) * 20);
    return 20 + Math.round((completedResources / totalResources) * 80);
  }

  function statusIcon(s: StepStatus): string {
    switch (s) {
      case 'done': return '✓';
      case 'error': return '✗';
      case 'running': return '◌';
      default: return '○';
    }
  }

  function resourceIcon(s: ResourceStatus): string {
    switch (s) {
      case 'created': case 'updated': case 'destroyed': return '✓';
      case 'creating': case 'updating': case 'destroying': case 'waiting': return '⟳';
      case 'error': return '✗';
      default: return '○';
    }
  }

  function friendlyAddress(addr: string): string {
    // aws_s3_bucket.frontend → S3 Bucket (frontend)
    const match = addr.match(/^aws_(\w+)\.(.+)$/);
    if (!match) return addr;
    const [, type_raw, name] = match;
    const typeMap: Record<string, string> = {
      's3_bucket': 'S3 Bucket',
      's3_bucket_policy': 'S3 Policy',
      's3_bucket_public_access_block': 'S3 Access Block',
      'cloudfront_distribution': 'CloudFront',
      'cloudfront_origin_access_identity': 'CloudFront OAI',
      'acm_certificate': 'ACM Certificate',
      'acm_certificate_validation': 'ACM Validation',
      'route53_record': 'Route53 Record',
      'lambda_function': 'Lambda',
      'sqs_queue': 'SQS Queue',
      'sns_topic': 'SNS Topic',
      'dynamodb_table': 'DynamoDB Table',
      'iam_role': 'IAM Role',
    };
    const friendly = typeMap[type_raw] || type_raw.replace(/_/g, ' ');
    return `${friendly} (${name})`;
  }
</script>

<div class="deploy-stream" class:deploy-stream--done={status === 'done'} class:deploy-stream--error={status === 'error'}>
  <!-- Progress bar -->
  <div class="ds-progress">
    <div class="ds-progress__track">
      <div
        class="ds-progress__fill"
        class:ds-progress__fill--done={status === 'done'}
        class:ds-progress__fill--error={status === 'error'}
        style="width: {progressPercent()}%"
      ></div>
    </div>
    <span class="ds-progress__label">
      {#if status === 'done'}
        Deploy complete
      {:else if status === 'error'}
        Failed
      {:else}
        {progressPercent()}%
      {/if}
    </span>
  </div>

  <!-- Steps -->
  <div class="ds-steps">
    {#each steps as step, i}
      <div class="ds-step" class:ds-step--running={step.status === 'running'} class:ds-step--done={step.status === 'done'} class:ds-step--error={step.status === 'error'}>
        <span class="ds-step__icon" class:ds-step__icon--spin={step.status === 'running'}>{statusIcon(step.status)}</span>
        <span class="ds-step__label">{step.label}</span>
        {#if step.error}<span class="ds-step__error">{step.error}</span>{/if}
      </div>
    {/each}
  </div>

  <!-- Resources (during apply) -->
  {#if resources.length > 0}
    <div class="ds-resources">
      <div class="ds-resources__header">
        Resources ({completedResources}/{resources.length})
      </div>
      {#each resources as res}
        <div class="ds-resource" class:ds-resource--active={res.status === 'creating' || res.status === 'updating' || res.status === 'destroying' || res.status === 'waiting'} class:ds-resource--done={res.status === 'created' || res.status === 'updated' || res.status === 'destroyed'}>
          <span class="ds-resource__icon" class:ds-resource__icon--spin={res.status === 'creating' || res.status === 'updating' || res.status === 'destroying' || res.status === 'waiting'}>{resourceIcon(res.status)}</span>
          <span class="ds-resource__name">{friendlyAddress(res.address)}</span>
          {#if res.elapsed}<span class="ds-resource__elapsed">{res.elapsed}</span>{/if}
          <span class="ds-resource__status">{res.status}</span>
        </div>
      {/each}
    </div>
  {/if}

  <!-- Log output -->
  {#if logs.length > 0}
    <details class="ds-logs">
      <summary>Output log ({logs.length} lines)</summary>
      <pre class="ds-logs__content">{logs.join('\n')}</pre>
    </details>
  {/if}

  <!-- Error -->
  {#if errorMsg}
    <div class="ds-error">{errorMsg}</div>
  {/if}

  <!-- Summary / Outputs -->
  {#if status === 'done' && Object.keys(outputs).length > 0}
    <div class="ds-outputs">
      {#each Object.entries(outputs) as [key, val]}
        <div class="ds-output">
          <span class="ds-output__key">{key}</span>
          <span class="ds-output__val">{val}</span>
        </div>
      {/each}
    </div>
  {/if}

  <!-- Actions -->
  <div class="ds-actions">
    {#if status === 'running'}
      <button type="button" class="btn-outline" onclick={cancel}>Cancel</button>
    {/if}
  </div>
</div>

<style>
.deploy-stream {
  border: 1px solid var(--dk-border-soft);
  border-radius: 0.65rem;
  padding: 1rem;
  background: var(--dk-surface-2, rgba(0,0,0,0.03));
  display: flex;
  flex-direction: column;
  gap: 0.75rem;
  animation: ds-in 0.3s ease-out both;
}
@keyframes ds-in {
  from { opacity: 0; transform: translateY(-6px); }
  to { opacity: 1; transform: translateY(0); }
}
.deploy-stream--done { border-color: color-mix(in srgb, var(--dk-green, #22c55e) 40%, var(--dk-border-soft)); }
.deploy-stream--error { border-color: color-mix(in srgb, var(--dk-red, #ef4444) 40%, var(--dk-border-soft)); }

.ds-progress { display: flex; align-items: center; gap: 0.75rem; }
.ds-progress__track {
  flex: 1; height: 6px; border-radius: 999px;
  background: var(--dk-border-soft); overflow: hidden;
}
.ds-progress__fill {
  height: 100%; border-radius: 999px;
  background: var(--dk-accent, #6366f1);
  transition: width 0.5s cubic-bezier(0.16, 1, 0.3, 1);
  position: relative; overflow: hidden;
}
.ds-progress__fill::after {
  content: ''; position: absolute; inset: 0;
  background: linear-gradient(90deg, transparent 0%, rgba(255,255,255,0.3) 50%, transparent 100%);
  animation: ds-shimmer 2s ease-in-out infinite;
}
@keyframes ds-shimmer { 0% { transform: translateX(-100%); } 100% { transform: translateX(100%); } }
.ds-progress__fill--done { background: var(--dk-green, #22c55e); }
.ds-progress__fill--done::after { animation: none; }
.ds-progress__fill--error { background: var(--dk-red, #ef4444); }
.ds-progress__fill--error::after { animation: none; }
.ds-progress__label { font-size: 0.8rem; color: var(--dk-text-muted); font-variant-numeric: tabular-nums; min-width: 3rem; text-align: right; }

.ds-steps { display: flex; gap: 1.5rem; }
.ds-step { display: flex; align-items: center; gap: 0.4rem; font-size: 0.85rem; }
.ds-step--done { color: var(--dk-green, #22c55e); }
.ds-step--error { color: var(--dk-red, #ef4444); }
.ds-step--running { color: var(--dk-accent, #6366f1); }
.ds-step__icon { font-size: 0.9rem; width: 1.2rem; text-align: center; }
.ds-step__icon--spin { animation: ds-spin 1s linear infinite; }
@keyframes ds-spin { to { transform: rotate(360deg); } }
.ds-step__label { font-weight: 550; }
.ds-step__error { font-size: 0.75rem; color: var(--dk-red, #ef4444); }

.ds-resources { display: flex; flex-direction: column; gap: 0.3rem; }
.ds-resources__header { font-size: 0.72rem; font-weight: 650; text-transform: uppercase; letter-spacing: 0.06em; color: var(--dk-text-muted); margin-bottom: 0.25rem; }
.ds-resource {
  display: grid; grid-template-columns: 1.2rem 1fr auto auto;
  align-items: center; gap: 0.4rem;
  font-size: 0.82rem; padding: 0.25rem 0;
  animation: ds-resource-in 0.2s ease-out both;
}
@keyframes ds-resource-in { from { opacity: 0; transform: translateX(-4px); } to { opacity: 1; transform: translateX(0); } }
.ds-resource--active { color: var(--dk-accent, #6366f1); }
.ds-resource--done { color: var(--dk-green, #22c55e); }
.ds-resource__icon { text-align: center; }
.ds-resource__icon--spin { animation: ds-spin 1s linear infinite; }
.ds-resource__name { font-weight: 500; }
.ds-resource__elapsed { font-size: 0.72rem; color: var(--dk-text-muted); font-variant-numeric: tabular-nums; }
.ds-resource__status { font-size: 0.68rem; text-transform: uppercase; letter-spacing: 0.04em; color: var(--dk-text-muted); }

.ds-logs { margin-top: 0.25rem; }
.ds-logs summary { font-size: 0.78rem; color: var(--dk-text-muted); cursor: pointer; }
.ds-logs__content {
  font-size: 0.72rem; font-family: ui-monospace, monospace;
  max-height: 12rem; overflow: auto;
  background: var(--dk-surface, #111); color: var(--dk-text-muted);
  padding: 0.5rem; border-radius: 0.4rem; margin-top: 0.35rem;
  white-space: pre-wrap; word-break: break-word;
}

.ds-error {
  color: var(--dk-red, #ef4444); font-size: 0.85rem;
  padding: 0.5rem 0.75rem; border-radius: 0.45rem;
  background: color-mix(in srgb, var(--dk-red, #ef4444) 10%, transparent);
  border: 1px solid color-mix(in srgb, var(--dk-red, #ef4444) 30%, transparent);
}

.ds-outputs {
  display: flex; flex-direction: column; gap: 0.3rem;
  padding: 0.6rem 0.75rem; border-radius: 0.45rem;
  background: color-mix(in srgb, var(--dk-green, #22c55e) 8%, transparent);
  border: 1px solid color-mix(in srgb, var(--dk-green, #22c55e) 25%, transparent);
}
.ds-output { display: flex; gap: 0.5rem; font-size: 0.82rem; }
.ds-output__key { font-weight: 600; color: var(--dk-text-muted); }
.ds-output__val { font-family: ui-monospace, monospace; word-break: break-all; }

.ds-actions { display: flex; justify-content: flex-end; }
</style>
