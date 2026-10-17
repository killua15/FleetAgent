import { useEffect, useState } from 'react';
import { ThemeProvider, useTheme } from './theme/ThemeProvider';
import { NavigationProvider, useNavigation } from './nav/NavigationProvider';
import { AuthProvider } from './auth/AuthProvider';
import { SignInGate } from './auth/SignInGate';
import { JumpPalette } from './components/JumpPalette';
import { Landing } from './screens/Landing';
import { MainView } from './screens/MainView';
import { AddAccount } from './screens/AddAccount';
import { AccountList } from './screens/AccountList';
import { AgentPanel } from './screens/AgentPanel';
import { SplitView } from './screens/SplitView';
import { GeneralSettings } from './screens/GeneralSettings';
import { ForgeSettings } from './screens/ForgeSettings';
import { TaskSettings } from './screens/TaskSettings';
import { TaskBoard } from './screens/TaskBoard';
import { UpdateBanner } from './components/UpdateBanner';
import { ReloginBanner } from './components/ReloginBanner';
import { NotificationBell } from './components/NotificationBell';
import { NotificationsPanel } from './components/NotificationsPanel';
import { setNotificationsOpen, useHasTopBar } from './lib/notifications';
import { listen } from '@tauri-apps/api/event';
import type { ScreenName } from './nav/NavigationProvider';
import { useBridge } from './bridge/useBridge';

// Inline rather than a module: rules that exist only to let the banners sit
// above a screen that expects the full viewport height.
const appShell = 'hangar-shell';
const appBody = 'hangar-body';
const appStack = 'hangar-stack';
import { Changes } from './screens/Changes';
import { RotationSettings } from './screens/RotationSettings';
import { ProjectSettings } from './screens/ProjectSettings';
import { SkillSettings } from './screens/SkillSettings';
import { SharedMemorySettings } from './screens/SharedMemorySettings';
import { GraphGroups } from './screens/GraphGroups';
import { PrReviews } from './screens/PrReviews';
import { GitPanel } from './screens/GitPanel';
import { Files } from './screens/Files';
import { Queue } from './screens/Queue';
import { StartHere } from './screens/StartHere';
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
/** El puente vive aquí, dentro de los proveedores y fuera del router: un cambio
 * de pantalla no debe cortar la conexión del móvil. */
function MobileBridge() {
  useBridge();
  return null;
}

function GlobalShortcuts() {
  const { toggleTheme } = useTheme();
  const { navigate } = useNavigation();
  const [jumpOpen, setJumpOpen] = useState(false);

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      const meta = e.metaKey || e.ctrlKey;
      if (meta && e.shiftKey && e.key.toLowerCase() === 'l') {
        e.preventDefault();
        toggleTheme();
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
  }, [toggleTheme]);

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
    case 'forge-settings':
      return <ForgeSettings />;
    case 'rotation-settings':
      return <RotationSettings />;
    case 'project-settings':
      return <ProjectSettings />;
    case 'skill-settings':
      return <SkillSettings />;
    case 'task-settings':
      return <TaskSettings />;
    case 'task-board':
      return <TaskBoard />;
    case 'shared-memory':
      return <SharedMemorySettings />;
    case 'graph-groups':
      return <GraphGroups />;
    case 'pr-reviews':
      return <PrReviews />;
    case 'git-panel':
      return <GitPanel />;
    case 'files':
      return <Files />;
    case 'queue':
      return <Queue />;
    case 'start-here':
      return <StartHere />;
    case 'changes':
      return <Changes />;
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

/** The bell for screens that don't render the top bar (PRs, Settings, Git…),
 * so the notification list is one click away from anywhere. */
function FloatingBell() {
  const hasTopBar = useHasTopBar();
  return hasTopBar ? null : <NotificationBell />;
}

/** The menu-bar icon's menu says where to go; the window owns the routes. */
function TrayBridge() {
  const { navigate } = useNavigation();
  useEffect(() => {
    const offNavigate = listen<{ screen: ScreenName; params: Record<string, unknown> }>('tray-navigate', (event) => {
      navigate(event.payload.screen, event.payload.params);
    });
    const offBell = listen('tray-open-notifications', () => setNotificationsOpen(true));
    return () => {
      offNavigate.then((f) => f());
      offBell.then((f) => f());
    };
  }, [navigate]);
  return null;
}

export default function App() {
  return (
    <AppErrorOverlay>
      <ThemeProvider>
        <TooltipProvider>
          <AuthProvider>
            {/* Above every screen and outside the router: an update is true
                wherever you happen to be, and it must not vanish because you
                navigated. */}
            <div className={appShell}>
              <UpdateBanner />
              {/* Inside the shell, under the banner: someone the gate turns
                  away because their build is too old still has to be able to
                  see and take the update that lets them in. */}
              <div className={appBody}>
                <SignInGate>
                  <NavigationProvider>
                    <GlobalShortcuts />
                    <MobileBridge />
                    {/* One element, not two siblings. `.hangar-body > *` gives
                        every direct child `height: 100%`, and neither the gate
                        (a fragment) nor the provider (a context) creates an
                        element of its own — so a bare banner beside the screen
                        became a full-height box with its content floating in
                        the middle, and pushed the whole app out of view. The
                        wrapper takes that 100% and shares it out: banner first
                        at its natural height, screen with the rest.

                        Inside the router, unlike the update banner above it:
                        the banner's button opens a terminal and navigates
                        there, so it needs the navigation it sits in. */}
                    <div className={appStack}>
                      <ReloginBanner />
                      <Screens />
                    </div>
                    {/* Global, over whatever screen is showing: notifications
                        are true everywhere, and a click can navigate anywhere. */}
                    <NotificationsPanel />
                    <FloatingBell />
                    <TrayBridge />
                  </NavigationProvider>
                </SignInGate>
              </div>
            </div>
          </AuthProvider>
        </TooltipProvider>
      </ThemeProvider>
    </AppErrorOverlay>
  );
}
