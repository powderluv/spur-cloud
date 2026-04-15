import { useState, useEffect } from 'react';
import { request } from '../api/client';

interface UsageSummary {
  total_gpu_seconds: number;
  total_gpu_hours: number;
  by_gpu_type: { gpu_type: string; gpu_seconds: number; gpu_hours: number; session_count: number }[];
}

interface UsageRecord {
  id: string;
  session_id: string;
  gpu_type: string;
  gpu_count: number;
  start_time: string;
  end_time: string | null;
  gpu_seconds: number;
}

async function fetchSummary(): Promise<UsageSummary> {
  return request<UsageSummary>('/billing/summary');
}

async function fetchUsage(): Promise<UsageRecord[]> {
  return request<UsageRecord[]>('/billing/usage');
}

export default function Billing() {
  const [summary, setSummary] = useState<UsageSummary | null>(null);
  const [records, setRecords] = useState<UsageRecord[]>([]);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    Promise.all([fetchSummary(), fetchUsage()])
      .then(([s, r]) => { setSummary(s); setRecords(r); })
      .finally(() => setLoading(false));
  }, []);

  if (loading) {
    return <div className="max-w-5xl mx-auto px-4 py-8 text-gray-400">Loading...</div>;
  }

  return (
    <div className="max-w-5xl mx-auto px-4 py-8">
      <h1 className="text-2xl font-bold text-white mb-6">Usage & Billing</h1>

      {/* Summary Cards */}
      {summary && (
        <div className="grid grid-cols-1 md:grid-cols-3 gap-4 mb-8">
          <div className="bg-gray-900 border border-gray-800 rounded-lg p-6">
            <p className="text-sm text-gray-400">Total GPU Hours (30d)</p>
            <p className="text-3xl font-bold text-white mt-1">
              {summary.total_gpu_hours.toFixed(1)}
            </p>
          </div>
          {summary.by_gpu_type.map(g => (
            <div key={g.gpu_type} className="bg-gray-900 border border-gray-800 rounded-lg p-6">
              <p className="text-sm text-gray-400 uppercase">{g.gpu_type}</p>
              <p className="text-3xl font-bold text-white mt-1">{g.gpu_hours.toFixed(1)}h</p>
              <p className="text-xs text-gray-500 mt-1">{g.session_count} sessions</p>
            </div>
          ))}
        </div>
      )}

      {/* Usage Records Table */}
      <h2 className="text-lg font-semibold text-white mb-4">Recent Usage</h2>
      <div className="bg-gray-900 border border-gray-800 rounded-lg overflow-x-auto">
        <table className="w-full">
          <thead>
            <tr className="text-left text-gray-500 text-sm border-b border-gray-800">
              <th className="pb-3 pl-4 pt-3">GPU</th>
              <th className="pb-3 pt-3">Count</th>
              <th className="pb-3 pt-3">Started</th>
              <th className="pb-3 pt-3">Ended</th>
              <th className="pb-3 pt-3">GPU Hours</th>
            </tr>
          </thead>
          <tbody>
            {records.length === 0 ? (
              <tr>
                <td colSpan={5} className="py-8 text-center text-gray-500">No usage records yet</td>
              </tr>
            ) : (
              records.map(r => (
                <tr key={r.id} className="border-b border-gray-800/50">
                  <td className="py-3 pl-4 font-mono text-sm uppercase">{r.gpu_type}</td>
                  <td className="py-3 text-sm">{r.gpu_count}</td>
                  <td className="py-3 text-sm text-gray-400">
                    {new Date(r.start_time).toLocaleString()}
                  </td>
                  <td className="py-3 text-sm text-gray-400">
                    {r.end_time ? new Date(r.end_time).toLocaleString() : <span className="text-green-400">running</span>}
                  </td>
                  <td className="py-3 text-sm font-mono">
                    {(r.gpu_seconds / 3600).toFixed(2)}
                  </td>
                </tr>
              ))
            )}
          </tbody>
        </table>
      </div>
    </div>
  );
}
