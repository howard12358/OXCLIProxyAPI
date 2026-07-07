# Keeper Public Entry Boundary

This document records the current boundary for automating `cpa-usage-keeper`
checks against the public deployment used with the embedded Rust data plane.

Checked on **July 7, 2026**.

## Scope

This is not a repository code-path guarantee. It is an environment boundary
record for the currently observed public keeper entrypoint:

- keeper public URL: `http://104.129.61.38:28081`

## What Was Verified

### Reachable from the public internet

- `GET /`
  - returned `200 OK`
  - returned the SPA HTML shell
- `GET /api/v1/auth/session`
  - returned `200 OK`
  - returned `{"authenticated":false}`
- `GET /api/v1/status`
  - returned `401 {"error":"authentication required"}`
- `GET /api/v1/usage/overview`
  - returned `401 {"error":"authentication required"}`
- `GET /api/v1/key-overview`
  - returned `401 {"error":"authentication required"}`
- `GET /api/v1/version`
  - returned `401 {"error":"authentication required"}`

These results show that the public keeper site is up and at least part of the
API surface is exposed and routed to a live application.

## What Blocked Automation

### Login automation is blocked at the public entrypoint

- `POST /api/v1/auth/login`
  - returned `403 {"error":"fetch request required"}`
- `POST /api/v1/auth/api-key-login`
  - returned `403 {"error":"fetch request required"}`

The same response was observed both:

- with a plain `curl` JSON request
- with extra browser-like headers such as:
  - `Origin`
  - `Referer`
  - `Sec-Fetch-Mode`
  - `Sec-Fetch-Site`
  - `X-Requested-With`

### Route-surface mismatch also exists

- `GET /api/v1/status/active`
  - returned `404 Not Found`
- `POST /api/v1/status/active`
  - returned `404 Not Found`

In the inspected local `cpa-usage-keeper` repository, `GET /api/v1/status/active`
is a real route. The public deployment not exposing it the same way means at
least one of these is true:

1. the deployed keeper build differs from the local repository version
2. the public entrypoint applies extra route filtering in front of keeper

## What This Means

The current public keeper boundary should be treated as:

- **keeper reachability can be smoke-tested**
- **authenticated data-page automation cannot currently be treated as a pure repo behavior**

For this repository, the minimal embedded smoke script only checks keeper
reachability (`GET /`) and does not attempt scripted public login.

## Most Likely Explanation

Based on the evidence, the strongest current inference is:

- there is an extra public-entry gate in front of keeper login endpoints, or
- the deployed keeper binary is not exactly the same route surface as the local
  repository state

The error string `fetch request required` was not found in the inspected local
`cpa-usage-keeper` repository, which makes a pure in-repo application behavior
less likely.

This remains an inference, not a confirmed infrastructure fact.

## Recommended Follow-Up

To make keeper login automation deterministic, one of these needs to happen:

1. verify the reverse-proxy / WAF / edge config in front of the public keeper
2. verify the exact deployed keeper image or binary version
3. provide an internal-only keeper URL that bypasses the public entry gate for
   automation checks
