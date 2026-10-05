// The third-party sites a sign-in reaches, faked in made-up TNG data:
// a login page that hands out a credential, and the few read-only
// endpoints each provider's probe calls. Only what the probes read is
// here; the shapes are the providers' own (see each provider's
// `probe.rs`). A handler gets the request and returns, or resolves to,
// `{status?, json? | html? | text?, headers?}`.

/// The credential each site accepts. Specs paste these, or expect a
/// browser login to capture them.
export const TNG = {
  slackToken: "xoxp-tng-picard",
  claudeSessionKey: "sk-ant-tng-picard",
  chatgptSessionCookie: "tng-chatgpt-session",
  chatgptAccessToken: "tng-chatgpt-access",
};

const cookies = (req) =>
  Object.fromEntries(
    String(req.headers.cookie ?? "")
      .split(";")
      .map((c) => c.trim().split("="))
      .filter(([k]) => k),
  );
const bearer = (req) => String(req.headers.authorization ?? "").replace(/^Bearer /, "");

function slack(req) {
  // Slack answers a bad token with HTTP 200 and `ok: false`.
  if (bearer(req) !== TNG.slackToken) return { json: { ok: false, error: "invalid_auth" } };
  const page = { ok: true, response_metadata: { next_cursor: "" } };
  switch (req.path) {
    case "/api/auth.test":
      return {
        json: {
          ok: true,
          url: "https://enterprise.slack.com/",
          team: "Enterprise",
          user: "picard",
          team_id: "T_ENTERPRISE",
          user_id: "U_PICARD",
        },
      };
    case "/api/users.list":
      return {
        json: {
          ...page,
          members: [
            { id: "U_PICARD", name: "picard", real_name: "Jean-Luc Picard" },
            { id: "U_RIKER", name: "riker", real_name: "William Riker" },
          ],
        },
      };
    case "/api/conversations.list": {
      const q = new URLSearchParams(req.query);
      if (q.get("types") === "im,mpim") {
        return { json: { ...page, channels: [{ id: "D_RIKER", is_im: true, user: "U_RIKER" }] } };
      }
      // Channels come in two pages, so a picker has progress to show.
      if (q.get("cursor") === "page-2") {
        return {
          json: {
            ...page,
            channels: [{ id: "C_TEN", name: "ten-forward", is_channel: true, num_members: 40 }],
          },
        };
      }
      return {
        json: {
          ok: true,
          response_metadata: { next_cursor: "page-2" },
          channels: [
            { id: "C_BRIDGE", name: "bridge", is_channel: true, is_member: true, num_members: 12 },
            { id: "C_ENG", name: "engineering", is_channel: true, is_private: true, is_member: true, num_members: 4 },
          ],
        },
      };
    }
    default:
      return { status: 404, json: { ok: false, error: "unknown_method" } };
  }
}

function claude(req) {
  if (req.path === "/login") {
    return {
      html: "<h1>Signed in to claude.ai (fake)</h1>",
      headers: { "set-cookie": `sessionKey=${TNG.claudeSessionKey}; Path=/; Secure; HttpOnly` },
    };
  }
  if (cookies(req).sessionKey !== TNG.claudeSessionKey) {
    return {
      status: 401,
      json: { type: "error", error: { type: "authentication_error", message: "Invalid authorization" } },
    };
  }
  switch (req.path) {
    case "/api/account":
      return {
        json: { uuid: "acct-picard", email_address: "picard@enterprise.test", full_name: "Jean-Luc Picard" },
      };
    case "/api/organizations":
      return { json: [{ uuid: "org-enterprise", name: "Enterprise" }] };
    case "/api/organizations/org-enterprise/chat_conversations":
      return {
        json: [
          { uuid: "conv-warp-core", name: "Warp core diagnostics", updated_at: "2026-09-30T12:00:00Z" },
          { uuid: "conv-tea", name: "Tea, Earl Grey, hot", updated_at: "2026-09-29T08:00:00Z" },
        ],
      };
    default:
      return { status: 404, json: { type: "error", error: { type: "not_found_error" } } };
  }
}

function chatgpt(req) {
  if (req.path === "/auth/login") {
    return {
      html: "<h1>Signed in to ChatGPT (fake)</h1>",
      headers: {
        "set-cookie": `__Secure-next-auth.session-token=${TNG.chatgptSessionCookie}; Path=/; Secure; HttpOnly`,
      },
    };
  }
  if (req.path === "/api/auth/session") {
    // What chatgpt.com's own page fetches: the bearer token, but only
    // for a signed-in browser.
    const signedIn = cookies(req)["__Secure-next-auth.session-token"] === TNG.chatgptSessionCookie;
    return { json: signedIn ? { user: { id: "user-picard" }, accessToken: TNG.chatgptAccessToken } : {} };
  }
  if (bearer(req) !== TNG.chatgptAccessToken) {
    return { status: 401, json: { detail: "Unauthorized" } };
  }
  switch (req.path) {
    case "/backend-api/me":
      return { json: { id: "user-picard", email: "picard@enterprise.test", name: "Jean-Luc Picard" } };
    case "/backend-api/conversations":
      return {
        json: {
          items: [{ id: "c-holodeck", title: "Holodeck safety protocols", update_time: 1790000000 }],
          total: 1,
          limit: 28,
          offset: 0,
        },
      };
    default:
      return { status: 404, json: { detail: "Not Found" } };
  }
}

export const FAKE_SITES = {
  "slack.com": slack,
  "claude.ai": claude,
  "chatgpt.com": chatgpt,
};
