import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { BrowserRouter } from 'react-router-dom';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import App from './App';
import './styles/index.css';

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      // Retry failed requests up to 3 times
      retry: 3,
      // Consider data stale after 5 seconds by default
      staleTime: 5_000,
      // Keep unused data in cache for 5 minutes
      gcTime: 5 * 60 * 1000,
      // Disabled: 60s master cache + WebSocket push make focus refetch harmful
      refetchOnWindowFocus: false,
    },
    mutations: {
      // Retry mutations once on network errors
      retry: 1,
    },
  },
});

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <BrowserRouter basename="/ctrl-modem">
        <App />
      </BrowserRouter>
    </QueryClientProvider>
  </StrictMode>
);
