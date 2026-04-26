import { Link, useNavigate } from 'react-router-dom';
import type { Session } from '../api/client';

interface Props {
  sessions: Session[];
}

const stateColors: Record<string, string> = {
  creating: 'text-blue-400',
  pending: 'text-yellow-400',
  running: 'text-green-400',
  stopping: 'text-orange-400',
  completed: 'text-gray-400',
  failed: 'text-red-400',
  cancelled: 'text-gray-500',
};

function timeAgo(dateStr: string): string {
  const diff = Date.now() - new Date(dateStr).getTime();
  const mins = Math.floor(diff / 60000);
  if (mins < 60) return `${mins}m ago`;
  const hrs = Math.floor(mins / 60);
  if (hrs < 24) return `${hrs}h ago`;
  return `${Math.floor(hrs / 24)}d ago`;
}

export default function SessionTable({ sessions }: Props) {
  const navigate = useNavigate();

  if (sessions.length === 0) {
    return (
      <div className="text-center text-gray-500 py-12">
        No sessions yet. <Link to="/sessions/new" className="text-blue-400 hover:underline">Launch one</Link>
      </div>
    );
  }

  return (
    <div className="overflow-x-auto">
      <table className="w-full">
        <thead>
          <tr className="text-left text-gray-500 text-sm border-b border-gray-800">
            <th className="pb-3 pl-4">Name</th>
            <th className="pb-3">GPU</th>
            <th className="pb-3">State</th>
            <th className="pb-3">Node</th>
            <th className="pb-3">SSH</th>
            <th className="pb-3">Created</th>
          </tr>
        </thead>
        <tbody>
          {sessions.map((s) => (
            <tr
              key={s.id}
              onClick={() => navigate(`/sessions/${s.id}`)}
              className="border-b border-gray-800/50 hover:bg-gray-800/50 cursor-pointer transition"
            >
              <td className="py-3 pl-4">
                <span className="text-blue-400 font-medium">{s.name}</span>
              </td>
              <td className="py-3">
                <span className="font-mono text-sm">
                  {s.gpu_count === 0 ? 'CPU' : `${s.gpu_count}x ${s.gpu_type}`}
                </span>
              </td>
              <td className="py-3">
                <span className={`font-medium ${stateColors[s.state] || 'text-gray-400'}`}>
                  {s.state}
                </span>
              </td>
              <td className="py-3 text-sm font-mono text-gray-400">{s.node_name || '-'}</td>
              <td className="py-3 text-sm">
                {s.ssh_enabled && s.ssh_port ? (
                  <code className="text-green-400">:{s.ssh_port}</code>
                ) : s.ssh_enabled ? (
                  <span className="text-yellow-400">pending</span>
                ) : '-'}
              </td>
              <td className="py-3 text-sm text-gray-500">{timeAgo(s.created_at)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
