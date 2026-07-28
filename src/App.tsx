import { useEffect, useState } from 'react';
import { ThemeProvider, useTheme } from './theme/ThemeProvider';
import { NavigationProvider, useNavigation } from './nav/NavigationProvider';
import { AuthProvider } from './auth/AuthProvider';
import { JumpPalette } from './components/JumpPalette';
import { Landing } from './screens/Landing';
import { MainView } from './screens/MainView';
import { AddAccount } from './screens/AddAccount';
import { AccountList } from './screens/AccountList';
import { AgentPanel } from './screens/AgentPanel';
import { SplitView } from './screens/SplitView';
import { GeneralSettings } from './screens/GeneralSettings';
import { TaskSettings } from './screens/TaskSettings';
import { TaskBoard } from './screens/TaskBoard';
import { RotationSettings } from './screens/RotationSettings';
import { SharedMemorySettings } from './screens/SharedMemorySettings';
import { PrReviews } from './screens/PrReviews';
import { GitPanel } from './screens/GitPanel';
import { Files } from './screens/Files';
import { Queue } from './screens/Queue';
import { GitHistoryView } from './screens/GitHistoryView';
import { TreeCanvas } from './screens/TreeCanvas';
import { TreeColumns } from './screens/TreeColumns';
import { TreeOutline } from './screens/TreeOutline';
import { ExecRun } from './screens/ExecRun';
import { Terminals } from './screens/Terminals';
import { CommandPalette } from './screens/CommandPalette';
import { Placeholder } from './components/Placeholder';
import { AppErrorOverlay } from './components/ErrorOverlay';
import { TooltipProvider } from './components/ui/tooltip';
import './styles/global.css';

/** App-wide keyboard shortcuts that don't belong to any one screen. */
function GlobalShortcuts() {
  const { toggleTheme } = useTheme();
  const { current, navigate } = useNavigation();
  const [jumpOpen, setJumpOpen] = useState(false);

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      const meta = e.metaKey || e.ctrlKey;
      if (meta && e.shiftKey && e.key.toLowerCase() === 'l') {
        e.preventDefault();
        toggleTheme();
      } else if (meta && !e.shiftKey && e.key.toLowerCase() === 'k') {
        e.preventDefault();
        if (current.screen !== 'command-palette') navigate('command-palette');
      } else if (meta && !e.shiftKey && e.key.toLowerCase() === 'j') {
        // ⌘J jumps to a running terminal from anywhere — including the Git
        // panel or the file viewer, which is exactly when navigating home first
        // and then hunting is most annoying.
        e.preventDefault();
        setJumpOpen((wasOpen) => !wasOpen);
      }
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [toggleTheme, navigate, current.screen]);

  return (
    <JumpPalette
      open={jumpOpen}
      onClose={() => setJumpOpen(false)}
      onJump={(id) => navigate('terminals', { focusTerminalId: id })}
    />
  );
}

function Screens() {
  const { current } = useNavigation();
  switch (current.screen) {
    case 'landing':
      return <Landing />;
    case 'main-view':
      return <MainView />;
    case 'add-account':
      return <AddAccount />;
    case 'account-list':
      return <AccountList />;
    case 'agent-panel':
      return <AgentPanel />;
    case 'split-view':
      return <SplitView />;
    case 'general-settings':
      return <GeneralSettings />;
    case 'rotation-settings':
      return <RotationSettings />;
    case 'task-settings':
      return <TaskSettings />;
    case 'task-board':
      return <TaskBoard />;
    case 'shared-memory':
      return <SharedMemorySettings />;
    case 'pr-reviews':
      return <PrReviews />;
    case 'git-panel':
      return <GitPanel />;
    case 'files':
      return <Files />;
    case 'queue':
      return <Queue />;
    case 'git-history':
      return <GitHistoryView />;
    case 'tree-canvas':
      return <TreeCanvas />;
    case 'tree-columns':
      return <TreeColumns />;
    case 'tree-outline':
      return <TreeOutline />;
    case 'exec-run':
      return <ExecRun />;
    case 'terminals':
      return <Terminals />;
    case 'command-palette':
      return <CommandPalette />;
    default:
      return <Placeholder screen={current.screen} />;
  }
}

export default function App() {
  return (
    <AppErrorOverlay>
      <ThemeProvider>
        <TooltipProvider>
          <AuthProvider>
            <NavigationProvider>
              <GlobalShortcuts />
              <Screens />
            </NavigationProvider>
          </AuthProvider>
        </TooltipProvider>
      </ThemeProvider>
    </AppErrorOverlay>
  );
}
