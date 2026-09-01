import React from 'react';
import { createRoot } from 'react-dom/client';
import '../app/globals.css';
import RenuxaApp from '../app/renuxa-app';

createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <RenuxaApp />
  </React.StrictMode>,
);
