import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import App from './App';
import './index.css';

// ?theme=light|dark forces the page theme (index.css keys off data-theme). The
// HC-800's on-screen view passes it this way: cog has no native light/dark
// switch, where the EA's webview sets the browser's preference itself.
const forced = new URLSearchParams(location.search).get('theme');
if (forced === 'light' || forced === 'dark') document.documentElement.dataset.theme = forced;

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
