import { useEffect } from 'react';
import { ThemeProvider, useTheme } from './theme/ThemeProvider';
import { NavigationProvider, useNavigation } from './nav/NavigationProvider';
import { Landing } from './screens/Landing';
import { MainView } from './screens/MainView';
import { AddAccount } from './screens/AddAccount';
import { AccountList } from './screens/AccountList';
import { AgentPanel } from './screens/AgentPanel';
import { SplitView } from './screens/SplitView';
import { GeneralSettings } from './screens/GeneralSettings';
import { RotationSettings } from './screens/RotationSettings';
import { SharedMemorySettings } from './screens/SharedMemorySettings';
import { Placeholder } from './components/Placeholder';
import { AppErrorOverlay } from './components/ErrorOverlay';
import './styles/global.css';

function ThemeShortcut() {
  const { toggleTheme } = useTheme();
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      const meta = e.metaKey || e.ctrlKey;
      if (meta && e.shiftKey && e.key.toLowerCase() === 'l') {
        e.preventDefault();
        toggleTheme();
      }
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [toggleTheme]);
  return null;
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
    case 'shared-memory':
      return <SharedMemorySettings />;
    default:
      return <Placeholder screen={current.screen} />;
  }
}

export default function App() {
  return (
    <AppErrorOverlay>
      <ThemeProvider>
        <NavigationProvider>
          <ThemeShortcut />
          <Screens />
        </NavigationProvider>
      </ThemeProvider>
    </AppErrorOverlay>
  );
}
