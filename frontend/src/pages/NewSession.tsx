import { useState, useEffect } from 'react';
import { useNavigate } from 'react-router-dom';
import { sessions, gpus, type GpuPool } from '../api/client';

const defaultImages = [
  { label: 'PyTorch (ROCm)', value: 'rocm/pytorch:latest' },
  { label: 'PyTorch (CUDA)', value: 'pytorch/pytorch:latest' },
  { label: 'TensorFlow (ROCm)', value: 'rocm/tensorflow:latest' },
  { label: 'Ubuntu 22.04', value: 'ubuntu:22.04' },
  { label: 'Custom', value: '' },
];

export default function NewSession() {
  const navigate = useNavigate();
  const [gpuPools, setGpuPools] = useState<GpuPool[]>([]);
  const [name, setName] = useState('');
  const [gpuType, setGpuType] = useState('none');
  const [gpuCount, setGpuCount] = useState(0);
  const [imagePreset, setImagePreset] = useState(defaultImages[0].value);
  const [customImage, setCustomImage] = useState('');
  const [sshEnabled, setSshEnabled] = useState(true);
  const [timeLimit, setTimeLimit] = useState(240);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState('');

  useEffect(() => {
    gpus.capacity().then(pools => {
      setGpuPools(pools);
      // Auto-select first GPU pool if available; otherwise stay CPU-only
      if (pools.length > 0) {
        setGpuType(pools[0].gpu_type);
        setGpuCount(1);
      }
    }).catch(() => {});
  }, []);

  const containerImage = imagePreset || customImage;

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!name || !containerImage) {
      setError('Please fill in all required fields');
      return;
    }
    setError('');
    setSubmitting(true);
    try {
      const session = await sessions.create({
        name,
        gpu_type: gpuType,
        gpu_count: gpuCount,
        container_image: containerImage,
        ssh_enabled: sshEnabled,
        time_limit_min: timeLimit,
      });
      navigate(`/sessions/${session.id}`);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : 'Failed to create session');
    } finally {
      setSubmitting(false);
    }
  };

  const selectedPool = gpuPools.find(p => p.gpu_type === gpuType);
  const maxGpus = selectedPool ? Math.min(8, selectedPool.available) : 8;

  return (
    <div className="max-w-2xl mx-auto px-4 py-8">
      <h1 className="text-2xl font-bold text-white mb-6">Launch Session</h1>

      <form onSubmit={handleSubmit} className="bg-gray-900 border border-gray-800 rounded-xl p-8 space-y-6">
        {/* Session Name */}
        <div>
          <label className="block text-sm font-medium text-gray-300 mb-1">Session Name</label>
          <input
            type="text"
            value={name}
            onChange={e => setName(e.target.value)}
            placeholder="my-training-run"
            className="w-full px-3 py-2 bg-gray-800 border border-gray-700 rounded-lg text-white focus:border-blue-500 focus:outline-none"
            required
          />
        </div>

        {/* Resource Type */}
        <div>
          <label className="block text-sm font-medium text-gray-300 mb-1">Resources</label>
          <div className="grid grid-cols-2 gap-3">
            {/* CPU-only option */}
            <button
              type="button"
              onClick={() => { setGpuType('none'); setGpuCount(0); }}
              className={`p-4 rounded-lg border text-left transition ${
                gpuType === 'none'
                  ? 'border-blue-500 bg-blue-900/20'
                  : 'border-gray-700 bg-gray-800 hover:border-gray-600'
              }`}
            >
              <div className="font-semibold text-white">CPU Only</div>
              <div className="text-sm text-gray-400">No GPU — compute only</div>
            </button>
            {/* GPU pool options */}
            {gpuPools.map(pool => (
              <button
                key={pool.gpu_type}
                type="button"
                onClick={() => { setGpuType(pool.gpu_type); if (gpuCount === 0) setGpuCount(1); }}
                className={`p-4 rounded-lg border text-left transition ${
                  gpuType === pool.gpu_type
                    ? 'border-blue-500 bg-blue-900/20'
                    : 'border-gray-700 bg-gray-800 hover:border-gray-600'
                }`}
              >
                <div className="font-semibold text-white uppercase">{pool.gpu_type}</div>
                <div className="text-sm text-gray-400">
                  {pool.available}/{pool.total} available — {Math.round(pool.memory_mb / 1024)} GB
                </div>
              </button>
            ))}
          </div>
        </div>

        {/* GPU Count — only shown for GPU sessions */}
        {gpuType !== 'none' && (
          <div>
            <label className="block text-sm font-medium text-gray-300 mb-1">
              GPUs: {gpuCount}
            </label>
            <input
              type="range"
              min={1}
              max={maxGpus}
              value={gpuCount}
              onChange={e => setGpuCount(parseInt(e.target.value))}
              className="w-full"
            />
            <div className="flex justify-between text-xs text-gray-500">
              <span>1</span>
              <span>{maxGpus}</span>
            </div>
          </div>
        )}

        {/* Container Image */}
        <div>
          <label className="block text-sm font-medium text-gray-300 mb-1">Container Image</label>
          <select
            value={imagePreset}
            onChange={e => setImagePreset(e.target.value)}
            className="w-full px-3 py-2 bg-gray-800 border border-gray-700 rounded-lg text-white focus:border-blue-500 focus:outline-none mb-2"
          >
            {defaultImages.map(img => (
              <option key={img.value} value={img.value}>{img.label}</option>
            ))}
          </select>
          {!imagePreset && (
            <input
              type="text"
              value={customImage}
              onChange={e => setCustomImage(e.target.value)}
              placeholder="docker.io/myrepo/myimage:tag"
              className="w-full px-3 py-2 bg-gray-800 border border-gray-700 rounded-lg text-white focus:border-blue-500 focus:outline-none"
              required
            />
          )}
        </div>

        {/* Time Limit */}
        <div>
          <label className="block text-sm font-medium text-gray-300 mb-1">Time Limit</label>
          <select
            value={timeLimit}
            onChange={e => setTimeLimit(parseInt(e.target.value))}
            className="w-full px-3 py-2 bg-gray-800 border border-gray-700 rounded-lg text-white focus:border-blue-500 focus:outline-none"
          >
            <option value={60}>1 hour</option>
            <option value={120}>2 hours</option>
            <option value={240}>4 hours</option>
            <option value={480}>8 hours</option>
            <option value={720}>12 hours</option>
            <option value={1440}>24 hours</option>
            <option value={4320}>3 days</option>
            <option value={10080}>7 days</option>
          </select>
        </div>

        {/* SSH Access */}
        <div className="flex items-center gap-3">
          <input
            type="checkbox"
            id="ssh"
            checked={sshEnabled}
            onChange={e => setSshEnabled(e.target.checked)}
            className="w-4 h-4"
          />
          <label htmlFor="ssh" className="text-sm text-gray-300">
            Enable SSH access (requires SSH keys in Settings)
          </label>
        </div>

        {error && <p className="text-red-400 text-sm">{error}</p>}

        <button
          type="submit"
          disabled={submitting}
          className="w-full py-3 bg-blue-600 hover:bg-blue-500 disabled:bg-blue-800 text-white rounded-lg font-semibold transition"
        >
          {submitting ? 'Launching...' : 'Launch Session'}
        </button>
      </form>
    </div>
  );
}
