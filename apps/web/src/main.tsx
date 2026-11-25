import { lazy, StrictMode, type ComponentType } from 'react';
import { QueryClientProvider } from '@tanstack/react-query';
import { queryClient } from './lib/query';
import { createRoot } from 'react-dom/client';
import { AuthGate } from './components/AuthGate';
import { ConfirmHost } from './components/Confirm';
import { TooltipHost } from './components/Tooltip';
import { BrowserRouter, Navigate, Route, Routes, useParams } from 'react-router';
import './index.css';
import './theme';
import './lib/editorPrefs';
import { Layout } from './components/Layout';
import { ChatPage } from './pages/ChatsPage';
import { HomePage } from './pages/HomePage';
import { SessionRedirect } from './pages/SessionRedirect';

/**
 * A route screen loaded on first visit (its own chunk): the home screen and chats load with
 * the app, projects, settings, harness and system pages only when opened. Layout and
 * SubmenuLayout wrap their outlets in Suspense.
 */
function page<M extends Record<string, unknown>, K extends keyof M>(load: () => Promise<M>, name: K) {
  return lazy(() => load().then((m) => ({ default: m[name] as ComponentType })));
}

const ProjectsPage = page(() => import('./pages/ProjectsPage'), 'ProjectsPage');
const ProjectPage = page(() => import('./pages/ProjectPage'), 'ProjectPage');
const ProjectSessionPage = page(() => import('./pages/ProjectPage'), 'ProjectSessionPage');
const GroupPage = page(() => import('./pages/GroupPage'), 'GroupPage');
const GroupSessionPage = page(() => import('./pages/GroupPage'), 'GroupSessionPage');
const SettingsLayout = page(() => import('./pages/settings/SettingsLayout'), 'SettingsLayout');
const SettingsIndex = page(() => import('./pages/settings/SettingsLayout'), 'SettingsIndex');
const AppearancePage = page(() => import('./pages/settings/AppearancePage'), 'AppearancePage');
const LanguagePage = page(() => import('./pages/settings/LanguagePage'), 'LanguagePage');
const AgentPage = page(() => import('./pages/settings/AgentPage'), 'AgentPage');
const KeyboardPage = page(() => import('./pages/settings/KeyboardPage'), 'KeyboardPage');
const GitAccessPage = page(() => import('./pages/settings/GitAccessPage'), 'GitAccessPage');
const UsersPage = page(() => import('./pages/settings/UsersPage'), 'UsersPage');
const StoragePage = page(() => import('./pages/settings/StoragePage'), 'StoragePage');
const HarnessLayout = page(() => import('./pages/HarnessPage'), 'HarnessLayout');
const HarnessIndex = page(() => import('./pages/HarnessPage'), 'HarnessIndex');
const HarnessDetail = page(() => import('./pages/HarnessPage'), 'HarnessDetail');
const HarnessMcp = page(() => import('./pages/HarnessMcp'), 'HarnessMcp');
const SystemLayout = page(() => import('./pages/SystemPage'), 'SystemLayout');
const SystemIndex = page(() => import('./pages/SystemPage'), 'SystemIndex');
const SystemSection = page(() => import('./pages/SystemPage'), 'SystemSection');

/** Old project tab URLs (sessions, files, git…) lead to the project itself. */
function ProjectRedirect() {
  const { id } = useParams();
  return <Navigate to={`/projects/${id}`} replace />;
}

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <AuthGate>
      <BrowserRouter>
        <Routes>
          <Route element={<Layout />}>
            <Route index element={<HomePage />} />
            <Route path="chats" element={<Navigate to="/" replace />} />
            <Route path="chats/:sid" element={<ChatPage />} />
            <Route path="projects" element={<ProjectsPage />} />
            <Route path="projects/:id/s/:sid" element={<ProjectSessionPage />} />
            <Route path="projects/:id" element={<ProjectPage />} />
            <Route path="projects/:id/*" element={<ProjectRedirect />} />
            <Route path="groups/:id" element={<GroupPage />} />
            <Route path="groups/:id/s/:sid" element={<GroupSessionPage />} />
            <Route path="settings" element={<SettingsLayout />}>
              <Route index element={<SettingsIndex />} />
              <Route path="workspace" element={<Navigate to="/settings/agent" replace />} />
              <Route path="appearance" element={<AppearancePage />} />
              <Route path="agent" element={<AgentPage />} />
              <Route path="git" element={<GitAccessPage />} />
              <Route path="users" element={<UsersPage />} />
              <Route path="storage" element={<StoragePage />} />
              <Route path="keyboard" element={<KeyboardPage />} />
              <Route path="language" element={<LanguagePage />} />
              <Route path="harness" element={<Navigate to="/harness" replace />} />
            </Route>
            <Route path="system" element={<SystemLayout />}>
              <Route index element={<SystemIndex />} />
              <Route path=":section" element={<SystemSection />} />
            </Route>
            <Route path="session/:id" element={<SessionRedirect />} />
            <Route path="harness" element={<HarnessLayout />}>
              <Route index element={<HarnessIndex />} />
              <Route path="mcp" element={<HarnessMcp />} />
              <Route path=":kind" element={<HarnessDetail />} />
            </Route>
            <Route path="*" element={<Navigate to="/" replace />} />
          </Route>
        </Routes>
        <ConfirmHost />
        <TooltipHost />
      </BrowserRouter>
      </AuthGate>
    </QueryClientProvider>
  </StrictMode>,
);
