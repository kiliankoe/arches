import { useEffect, useState } from "react";
import { Route, Routes } from "react-router";
import "./App.css";
import { api, errorMessage, formatVersion, type Status } from "./api";

export default function App() {
  return (
    <Routes>
      <Route path="/" element={<Home />} />
    </Routes>
  );
}

function Home() {
  const [status, setStatus] = useState<Status | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    api
      .getStatus()
      .then((loaded) => !cancelled && setStatus(loaded))
      .catch((e) => !cancelled && setError(errorMessage(e)));
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <main className="app">
      <h1>arches</h1>
      {error ? (
        <p className="error">Could not reach the server: {error}</p>
      ) : (
        <p>{formatVersion(status)}</p>
      )}
    </main>
  );
}
