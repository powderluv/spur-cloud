import { useState, useEffect } from 'react';
import { useParams, useNavigate } from 'react-router-dom';
import { sessions, terminalWsUrl, type Session } from '../api/client';
import Terminal from '../components/Terminal';

export default function SessionDetail() {
  const { id } = useParams<{ id: string }>();
  const navigate = useNavigate();
  const [session, setSession] = useState<Session | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [copiedSsh, setCopiedSsh] = useState(false);

  useEffect(() => {
    if (!id) return;
    const refresh = () => {
      sessions.get(id).then(setSession).catch(() => navigate('/'));
    };
    refresh();
    const interval = setInterval(refresh, 5000);
    return () => clearInterval(interval);
  }, [id, navigate]);

  if (!session) {
    return <div className="max-w-5xl mx-auto px-4 py-8 text-gray-400">Loading...</div>;
  }

  const isRunning = session.state === 'running';
  const isTerminal = ['completed', 'failed', 'cancelled'].includes(session.state);
  const sshCommand = session.ssh_enabled && session.ssh_port
    ? `ssh -p ${session.ssh_port} root@${session.ssh_host || session.node_name || '<node>'}`
    : null;

  const handleDelete = async () => {
    if (!confirm('Terminate this session?')) return;
    setDeleting(true);
    try {
      await sessions.delete(session.id);
      navigate('/');
    } catch {
      setDeleting(false);
    }
  };

  const copyToClipboard = (text: string) => {
    navigator.clipboard.writeText(text);
    setCopiedSsh(true);
    setTimeout(() => setCopiedSsh(false), 2000);
  };

  return (
    <div className="max-w-5xl mx-auto px-4 py-8">
      {/* Header */}
      <div className="flex items-center justify-between mb-6">
        <div>
          <h1 className="text-2xl font-bold text-white">{session.name}</h1>
          <p className="text-gray-400 text-sm">
            Job #{session.spur_job_id || 'pending'}
            {session.node_name && <span> on <span className="text-gray-300">{session.node_name}</span></span>}
          </p>
        </div>
        <div className="flex gap-3">
          {!isTerminal && (
            <button
              onClick={handleDelete}
              disabled={deleting}
              className="px-4 py-2 bg-red-600 hover:bg-red-500 disabled:bg-red-800 text-white rounded-lg text-sm font-medium transition"
            >
              {deleting ? 'Stopping...' : 'Terminate'}
            </button>
          )}
        </div>
      </div>

      {/* Session Info */}
      <div className="bg-gray-900 border border-gray-800 rounded-lg p-6 mb-6">
        <div className="grid grid-cols-2 md:grid-cols-4 gap-4">
          <InfoItem label="State" value={session.state} highlight={isRunning ? 'green' : isTerminal ? 'red' : 'yellow'} />
          <InfoItem label="GPU" value={`${session.gpu_count}x ${session.gpu_type}`} />
          <InfoItem label="Node" value={session.node_name || 'pending'} />
          <InfoItem label="Time Limit" value={`${session.time_limit_min} min`} />
          <InfoItem label="Created" value={new Date(session.created_at).toLocaleString()} />
          {session.started_at && (
            <InfoItem label="Started" value={new Date(session.started_at).toLocaleString()} />
          )}
          {session.ended_at && (
            <InfoItem label="Ended" value={new Date(session.ended_at).toLocaleString()} />
          )}
          {session.container_image && (
            <InfoItem label="Image" value={session.container_image} />
          )}
        </div>
      </div>

      {/* Error Message */}
      {session.error_message && (
        <div className="bg-red-950 border border-red-800 rounded-lg p-6 mb-6">
          <h2 className="text-lg font-semibold text-red-400 mb-2">Error</h2>
          <pre className="text-red-300 text-sm font-mono whitespace-pre-wrap">{session.error_message}</pre>
        </div>
      )}

      {/* SSH Access */}
      {sshCommand && isRunning && (
        <div className="bg-gray-900 border border-gray-800 rounded-lg p-6 mb-6">
          <h2 className="text-lg font-semibold text-white mb-3">SSH Access</h2>
          <div className="flex items-center gap-3 bg-gray-950 rounded-lg p-4 border border-gray-700">
            <code className="text-green-400 text-sm font-mono flex-1 select-all">{sshCommand}</code>
            <button
              onClick={() => copyToClipboard(sshCommand)}
              className="px-3 py-1.5 bg-gray-700 hover:bg-gray-600 text-gray-300 rounded text-xs font-medium transition whitespace-nowrap"
            >
              {copiedSsh ? 'Copied!' : 'Copy'}
            </button>
          </div>
        </div>
      )}

      {/* Web Terminal */}
      {isRunning && (
        <div className="mb-6">
          <h2 className="text-lg font-semibold text-white mb-3">Web Terminal</h2>
          <Terminal wsUrl={terminalWsUrl(session.id)} />
        </div>
      )}

      {/* Job Examples */}
      {isRunning && session.node_name && (
        <div className="bg-gray-900 border border-gray-800 rounded-lg p-6 mb-6">
          <h2 className="text-lg font-semibold text-white mb-3">Running Jobs on Your Session</h2>
          <p className="text-gray-400 text-sm mb-4">
            Your session has {session.gpu_count}x {session.gpu_type} GPU(s) allocated.
            Use the web terminal above or SSH to run commands. Here are some examples:
          </p>

          <div className="space-y-4">
            <CodeBlock
              title="Check GPU status"
              code="rocm-smi"
              onCopy={copyToClipboard}
            />
            <CodeBlock
              title="Run a PyTorch script"
              code={`python3 -c "import torch; print(f'GPUs: {torch.cuda.device_count()}'); x = torch.randn(1000,1000, device='cuda'); print(f'Tensor on {x.device}: {x.shape}')"`}
              onCopy={copyToClipboard}
            />
            <CodeBlock
              title="Submit a batch job via Spur CLI"
              code={`srun --gres=gpu:${session.gpu_type}:${session.gpu_count} --partition=gpu python3 train.py`}
              onCopy={copyToClipboard}
            />
            <CodeBlock
              title="Interactive shell with GPU"
              code={`srun --gres=gpu:${session.gpu_type}:1 --partition=gpu --pty bash`}
              onCopy={copyToClipboard}
            />
            <CodeBlock
              title="Submit a batch script"
              code={`cat << 'EOF' > job.sh
#!/bin/bash
#SBATCH --job-name=my-training
#SBATCH --partition=gpu
#SBATCH --gres=gpu:${session.gpu_type}:${session.gpu_count}
#SBATCH --time=04:00:00

echo "Running on $(hostname) with $SPUR_JOB_GPUS GPUs"
rocm-smi
python3 train.py --epochs 100
EOF

sbatch job.sh`}
              onCopy={copyToClipboard}
            />
          </div>
        </div>
      )}
    </div>
  );
}

function InfoItem({ label, value, highlight }: { label: string; value: string; highlight?: string }) {
  const color = highlight === 'green' ? 'text-green-400' :
    highlight === 'red' ? 'text-red-400' :
    highlight === 'yellow' ? 'text-yellow-400' : 'text-gray-200';
  return (
    <div>
      <p className="text-xs text-gray-500 uppercase">{label}</p>
      <p className={`text-sm ${color} truncate`} title={value}>{value}</p>
    </div>
  );
}

function CodeBlock({ title, code, onCopy }: { title: string; code: string; onCopy: (s: string) => void }) {
  const [copied, setCopied] = useState(false);
  const handleCopy = () => {
    onCopy(code);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  return (
    <div>
      <p className="text-sm text-gray-300 mb-1">{title}</p>
      <div className="flex items-start gap-2 bg-gray-950 rounded-lg p-3 border border-gray-700">
        <pre className="text-green-400 text-xs font-mono flex-1 whitespace-pre-wrap overflow-x-auto">{code}</pre>
        <button
          onClick={handleCopy}
          className="px-2 py-1 bg-gray-700 hover:bg-gray-600 text-gray-300 rounded text-xs font-medium transition whitespace-nowrap flex-shrink-0"
        >
          {copied ? 'Copied!' : 'Copy'}
        </button>
      </div>
    </div>
  );
}
