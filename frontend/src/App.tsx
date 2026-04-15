import { BrowserRouter, Routes, Route, Navigate } from 'react-router-dom';
import { useState, useEffect, createContext, useContext } from 'react';
import Login from './pages/Login';
import Dashboard from './pages/Dashboard';
import NewSession from './pages/NewSession';
import SessionDetail from './pages/SessionDetail';
import Settings from './pages/Settings';
import Billing from './pages/Billing';
import Navbar from './components/Navbar';
import {
  clearSession,
  consumeOAuthCallbackSession,
  getAccessToken,
  getStoredUser,
  setAccessToken,
  setStoredUser,
  type SessionUser,
} from './auth/session';

interface AuthContextType {
  token: string | null;
  user: SessionUser | null;
  login: (token: string, user: AuthContextType['user']) => void;
  logout: () => void;
}

export const AuthContext = createContext<AuthContextType>({
  token: null,
  user: null,
  login: () => {},
  logout: () => {},
});

export function useAuth() {
  return useContext(AuthContext);
}

function ProtectedRoute({ children }: { children: React.ReactNode }) {
  const { token } = useAuth();
  if (!token) return <Navigate to="/login" />;
  return <>{children}</>;
}

export default function App() {
  const [token, setToken] = useState<string | null>(() => getAccessToken());
  const [user, setUser] = useState<AuthContextType['user']>(() => getStoredUser());

  // Handle OAuth callback token from URL fragment
  useEffect(() => {
    const callbackSession = consumeOAuthCallbackSession();
    if (callbackSession) {
      setToken(callbackSession.token);
      setAccessToken(callbackSession.token);
      if (callbackSession.user) {
        setUser(callbackSession.user);
        setStoredUser(callbackSession.user);
      }
    }
  }, []);

  const login = (newToken: string, newUser: AuthContextType['user']) => {
    setToken(newToken);
    setUser(newUser);
    setAccessToken(newToken);
    if (newUser) setStoredUser(newUser);
  };

  const logout = () => {
    setToken(null);
    setUser(null);
    clearSession();
  };

  return (
    <AuthContext.Provider value={{ token, user, login, logout }}>
      <BrowserRouter>
        <div className="min-h-screen bg-gray-950 text-gray-100">
          {token && <Navbar />}
          <main className={token ? 'pt-16' : ''}>
            <Routes>
              <Route path="/login" element={<Login />} />
              <Route path="/" element={<ProtectedRoute><Dashboard /></ProtectedRoute>} />
              <Route path="/sessions/new" element={<ProtectedRoute><NewSession /></ProtectedRoute>} />
              <Route path="/sessions/:id" element={<ProtectedRoute><SessionDetail /></ProtectedRoute>} />
              <Route path="/settings" element={<ProtectedRoute><Settings /></ProtectedRoute>} />
              <Route path="/billing" element={<ProtectedRoute><Billing /></ProtectedRoute>} />
              <Route path="*" element={<Navigate to="/" />} />
            </Routes>
          </main>
        </div>
      </BrowserRouter>
    </AuthContext.Provider>
  );
}
