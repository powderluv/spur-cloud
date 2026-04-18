import {
  clearSession,
  getAccessToken,
  type SessionUser,
} from '../auth/session';

const API_BASE = '/api';

export async function request<T>(path: string, options: RequestInit = {}): Promise<T> {
  const token = getAccessToken();
  const headers: Record<string, string> = {
    'Content-Type': 'application/json',
    ...(options.headers as Record<string, string> || {}),
  };
  if (token) {
    headers['Authorization'] = `Bearer ${token}`;
  }

  const resp = await fetch(`${API_BASE}${path}`, { ...options, headers });

  if (resp.status === 401) {
    clearSession();
    window.location.href = '/login';
    throw new Error('Unauthorized');
  }

  if (!resp.ok) {
    const text = await resp.text();
    throw new Error(text || `HTTP ${resp.status}`);
  }

  if (resp.status === 204) return {} as T;
  return resp.json();
}

// Auth
export interface AuthResponse {
  token: string;
  user: SessionUser;
}

export interface Provider {
  name: string;
  enabled: boolean;
  authorize_url: string;
}

export const auth = {
  register: (email: string, username: string, password: string) =>
    request<AuthResponse>('/auth/register', {
      method: 'POST',
      body: JSON.stringify({ email, username, password }),
    }),
  login: (email: string, password: string) =>
    request<AuthResponse>('/auth/login', {
      method: 'POST',
      body: JSON.stringify({ email, password }),
    }),
  providers: () => request<{ providers: Provider[] }>('/auth/providers'),
};

// Sessions
export interface Session {
  id: string;
  name: string;
  state: string;
  gpu_type: string;
  gpu_count: number;
  container_image: string;
  ssh_enabled: boolean;
  ssh_host: string | null;
  ssh_port: number | null;
  spur_job_id: number | null;
  time_limit_min: number;
  created_at: string;
  started_at: string | null;
  ended_at: string | null;
  node_name: string | null;
  error_message: string | null;
}

export interface CreateSession {
  name: string;
  gpu_type: string;
  gpu_count: number;
  container_image: string;
  ssh_enabled: boolean;
  time_limit_min: number;
  partition?: string;
}

export const sessions = {
  list: (state?: string) =>
    request<Session[]>(`/sessions${state ? `?state=${state}` : ''}`),
  get: (id: string) => request<Session>(`/sessions/${id}`),
  create: (data: CreateSession) =>
    request<Session>('/sessions', { method: 'POST', body: JSON.stringify(data) }),
  delete: (id: string) =>
    request<void>(`/sessions/${id}`, { method: 'DELETE' }),
};

// GPUs
export interface GpuPool {
  gpu_type: string;
  total: number;
  available: number;
  allocated: number;
  memory_mb: number;
  nodes: { name: string; total_gpus: number; available_gpus: number; state: string }[];
}

export const gpus = {
  capacity: () => request<GpuPool[]>('/gpus'),
};

// SSH Keys
export interface SshKey {
  id: string;
  name: string;
  public_key: string;
  fingerprint: string;
  created_at: string;
}

export const sshKeys = {
  list: () => request<SshKey[]>('/users/me/ssh-keys'),
  add: (name: string, public_key: string) =>
    request<SshKey>('/users/me/ssh-keys', {
      method: 'POST',
      body: JSON.stringify({ name, public_key }),
    }),
  delete: (id: string) =>
    request<void>(`/users/me/ssh-keys/${id}`, { method: 'DELETE' }),
};

// User
export interface UserProfile {
  id: string;
  email: string;
  username: string;
  display_name: string | null;
  avatar_url: string | null;
  is_admin: boolean;
  created_at: string;
}

export const users = {
  me: () => request<UserProfile>('/users/me'),
};

// WebSocket terminal URL (passes current access token as query param since browser WS API can't set headers)
export function terminalWsUrl(sessionId: string): string {
  const proto = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
  const token = getAccessToken();
  return `${proto}//${window.location.host}/api/sessions/${sessionId}/terminal?token=${token}`;
}
