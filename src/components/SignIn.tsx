import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api, type Account, type ServerState } from "../lib/api";
import { Spinner } from "./Spinner";

/**
 * The gate.
 *
 * Three things have to be settled before anyone can read: where the library
 * is, whether it is there, and who you are. They are asked in that order,
 * because each is meaningless without the one before it.
 *
 * On a server with no accounts this becomes a registration form instead of a
 * sign-in one. The first person to arrive is the administrator — someone has
 * to be able to set the inference server's address, and a library nobody can
 * configure is a library nobody can use.
 */
export function SignIn({ onSignedIn }: { onSignedIn: (account: Account) => void }) {
  const [address, setAddress] = useState("");
  const [server, setServer] = useState<ServerState | null>(null);
  const [checking, setChecking] = useState(true);

  const [email, setEmail] = useState("");
  const [displayName, setDisplayName] = useState("");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api.serverAddress().then(setAddress).catch(() => setAddress(""));
  }, []);

  const check = async () => {
    setChecking(true);
    setError(null);
    try {
      setServer(await api.serverState());
    } catch (e) {
      setServer(null);
      setError(String(e));
    } finally {
      setChecking(false);
    }
  };

  // Re-checked whenever the address settles, so the form below always
  // describes the server it is actually going to talk to.
  useEffect(() => {
    if (!address) return;
    const timer = setTimeout(check, 300);
    return () => clearTimeout(timer);
  }, [address]);

  const trustCertificate = async () => {
    const chosen = await open({
      multiple: false,
      filters: [{ name: "Certificate", extensions: ["pem", "crt", "cer"] }],
    });
    if (typeof chosen !== "string") return;

    setError(null);
    try {
      await api.trustServerCertificate(chosen);
      await check();
    } catch (e) {
      setError(String(e));
    }
  };

  const changeAddress = async (next: string) => {
    setAddress(next);
    try {
      await api.setServerAddress(next);
    } catch (e) {
      setError(String(e));
    }
  };

  /**
   * Which form is showing.
   *
   * An empty server starts on registration, because there is nothing to sign
   * in to yet. Otherwise it starts on sign-in and the reader can switch, if
   * the server allows new accounts at all.
   */
  const [creating, setCreating] = useState(false);
  useEffect(() => {
    if (server?.needs_setup) setCreating(true);
  }, [server?.needs_setup]);

  const registering = creating && server?.registration_open !== false;
  const canSwitch = server?.registration_open === true && !server.needs_setup;

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const account = registering
        ? await api.register(email, displayName, password)
        : await api.signIn(email, password);
      onSignedIn(account);
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="flex h-full items-center justify-center bg-paper p-6 text-ink">
      <div className="panel-in w-full max-w-sm">
        <h1 className="font-serif text-2xl">Reading Companion</h1>
        <p className="mt-1 text-sm text-ink-soft">
          {server?.needs_setup
            ? "This library has no accounts yet. The first one is yours, and it is the administrator."
            : registering
              ? "Create an account on this library."
              : "Sign in to your library."}
        </p>

        <form onSubmit={submit} className="mt-6">
          <label className="block text-sm font-medium" htmlFor="server">
            Library server
          </label>
          <input
            id="server"
            value={address}
            onChange={(e) => changeAddress(e.target.value)}
            placeholder="http://127.0.0.1:7878"
            spellCheck={false}
            className="mt-1.5 w-full rounded border border-rule bg-transparent px-3 py-2 font-mono text-sm outline-none focus:border-accent"
          />

          <p className="mt-1.5 flex items-center gap-2 text-xs">
            {checking ? (
              <span className="text-ink-soft">Looking for it…</span>
            ) : server ? (
              <span className="text-emerald-700">
                <span className="mr-1.5 inline-block h-2 w-2 rounded-full bg-emerald-600" />
                {server.encrypted ? "Found it, encrypted" : "Found it"}
              </span>
            ) : (
              <span className="text-red-600">
                <span className="mr-1.5 inline-block h-2 w-2 rounded-full bg-red-500" />
                Nothing answered there
              </span>
            )}
          </p>

          {/* An https address that will not connect is usually a certificate
              the machine has not been told to trust, and the underlying error
              says so in language nobody should have to read. */}
          {!checking && !server && address.startsWith("https") && (
            <div className="mt-2 rounded border border-rule bg-paper-dim px-3 py-2 text-xs">
              <p className="text-ink-soft">
                If this server made its own certificate, this machine has to be
                told to trust it once.
              </p>
              <button
                type="button"
                onClick={trustCertificate}
                className="mt-1.5 text-accent hover:underline"
              >
                Trust a certificate…
              </button>
              <p className="mt-1 text-ink-soft">
                The installer wrote <code>server-cert.pem</code> beside itself.
              </p>
            </div>
          )}

          {server?.trusting_own_certificate && (
            <p className="mt-1.5 text-xs text-ink-soft">
              Trusting this server's own certificate.{" "}
              <button
                type="button"
                onClick={async () => {
                  await api.forgetServerCertificate().catch(() => {});
                  check();
                }}
                className="text-accent hover:underline"
              >
                Forget it
              </button>
            </p>
          )}

          {!checking && server && !server.encrypted && !address.includes("127.0.0.1") && (
            <p className="mt-1.5 text-xs text-amber-600">
              Not encrypted. Anything on the network between here and there can
              read what you send, including your password.
            </p>
          )}

          <div className="mt-5 border-t border-rule pt-5">
            <label className="block text-sm font-medium" htmlFor="email">
              Email
            </label>
            <input
              id="email"
              type="email"
              autoComplete="username"
              value={email}
              onChange={(e) => setEmail(e.target.value)}
              disabled={!server}
              className="mt-1.5 w-full rounded border border-rule bg-transparent px-3 py-2 text-sm outline-none focus:border-accent disabled:opacity-50"
            />

            {registering && (
              <>
                <label className="mt-3 block text-sm font-medium" htmlFor="name">
                  Name <span className="font-normal text-ink-soft">— optional</span>
                </label>
                <input
                  id="name"
                  value={displayName}
                  onChange={(e) => setDisplayName(e.target.value)}
                  className="mt-1.5 w-full rounded border border-rule bg-transparent px-3 py-2 text-sm outline-none focus:border-accent"
                />
              </>
            )}

            <label className="mt-3 block text-sm font-medium" htmlFor="password">
              Password
            </label>
            <input
              id="password"
              type="password"
              autoComplete={registering ? "new-password" : "current-password"}
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              disabled={!server}
              className="mt-1.5 w-full rounded border border-rule bg-transparent px-3 py-2 text-sm outline-none focus:border-accent disabled:opacity-50"
            />
            {registering && (
              <p className="mt-1.5 text-xs text-ink-soft">
                At least 10 characters. Length is the only thing that reliably
                helps, so there are no rules about symbols.
              </p>
            )}
          </div>

          {error && (
            <p className="mt-4 rounded border border-red-500/30 bg-red-500/5 px-3 py-2 text-xs text-red-600">
              {error}
            </p>
          )}

          <button
            type="submit"
            disabled={busy || !server || !email || !password}
            className="mt-5 flex w-full items-center justify-center gap-2 rounded bg-accent px-4 py-2 text-sm text-paper disabled:opacity-40"
          >
            {busy && <Spinner size={16} />}
            {server?.needs_setup
              ? "Create the library"
              : registering
                ? "Create the account"
                : "Sign in"}
          </button>

          {canSwitch && (
            <button
              type="button"
              onClick={() => {
                setCreating((c) => !c);
                setError(null);
              }}
              className="mt-3 w-full text-center text-xs text-ink-soft hover:text-accent"
            >
              {creating
                ? "I already have an account"
                : "Create an account on this library"}
            </button>
          )}
        </form>

        {server && !server.registration_open && (
          <p className="mt-4 text-xs text-ink-soft">
            This library is not accepting new accounts. Its administrator can
            turn that on in Settings.
          </p>
        )}

        <p className="mt-5 text-xs text-ink-soft">
          Your books stay on the server you name above. Nothing is sent
          anywhere else.
        </p>
      </div>
    </div>
  );
}
