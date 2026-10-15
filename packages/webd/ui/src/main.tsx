import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import App from './App';
import './index.css';

// On-panel day/night override. The on-panel view has no OS light/dark signal, so
// the display settings can force a theme; S95ohc-display passes it as ?theme=dark|
// light. The CSS already keys off [data-theme]; 'auto' (or no param) just falls
// through to prefers-color-scheme. Harmless in a normal browser.
const forcedTheme = new URLSearchParams(location.search).get('theme');
if (forcedTheme === 'dark' || forcedTheme === 'light') {
  document.documentElement.setAttribute('data-theme', forcedTheme);
}

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
