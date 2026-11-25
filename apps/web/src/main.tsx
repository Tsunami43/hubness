import { StrictMode } from 'react';
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
import { ProjectsPage } from './pages/ProjectsPage';
import { ProjectPage, ProjectSessionPage } from './pages/ProjectPage';
import { GroupPage, GroupSessionPage } from './pages/GroupPage';
import { SettingsIndex, SettingsLayout } from './pages/settings/SettingsLayout';
import { AppearancePage } from './pages/settings/AppearancePage';
import { LanguagePage } from './pages/settings/LanguagePage';
import { AgentPage } from './pages/settings/AgentPage';
import { KeyboardPage } from './pages/settings/KeyboardPage';
import { GitAccessPage } from './pages/settings/GitAccessPage';
import { HarnessDetail, HarnessIndex, HarnessLayout } from './pages/HarnessPage';
import { HarnessMcp } from './pages/HarnessMcp';
import { HomePage } from './pages/HomePage';
import { SystemIndex, SystemLayout, SystemSection } from './pages/SystemPage';
import { SessionRedirect } from './pages/SessionRedirect';

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
