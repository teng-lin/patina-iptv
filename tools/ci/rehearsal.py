#!/usr/bin/env python3
"""Create and remove the throwaway tag of a release rehearsal.

A rehearsal runs the whole publish job against a draft release on a tag named
`rehearsal-...`, then removes both. This script owns the two ends of that:
it makes the tag, and afterwards deletes the draft and the tag. It only ever
touches what the rehearsal provably owns: a lightweight tag with the
`rehearsal-` prefix that points at the expected commit, and draft releases of
that tag. A tag elsewhere, an annotated tag, a public release, or a draft whose
tag is gone is refused and left exactly as it was.

Both commands converge, so a rerun after any failure is safe: `create` accepts
a tag that already points at the expected commit, and `cleanup` removes
whatever an interrupted run left and succeeds when nothing is left.

Absence is only ever read from a successful answer: an empty list from GitHub.
A failed call (no token, no network, 401, 403, 404, 5xx) is an error and
nothing more is done. GitHub answers 404 for a repository the token cannot
see, so a 404 never means "the tag is not there".

Usage (with GH_TOKEN and GH_REPO=owner/name in the environment):
  rehearsal.py create TAG EXPECTED_SHA    make lightweight tag TAG at commit EXPECTED_SHA
  rehearsal.py cleanup TAG EXPECTED_SHA   delete the draft release(s) of TAG, then TAG itself,
                                          and check that neither is left

TAG must start with `rehearsal-`; EXPECTED_SHA is a full 40-digit commit SHA.
"""
import json
import os
import re
import subprocess
import sys

PREFIX = "rehearsal-"
TAG = re.compile(r"rehearsal-[A-Za-z0-9]+(?:[._-][A-Za-z0-9]+)*")
SHA = re.compile(r"[0-9a-f]{40}")
REPOSITORY = re.compile(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+")
STATUS = re.compile(r"HTTP/[0-9.]+ ([0-9]{3})\b")
PAGE = 100
# Far more than a repository holds; it only stops a listing that never ends.
MAX_PAGES = 200


def request(method, path, fields=None, expect=(200,)):
    """One GitHub API call: (HTTP status, decoded body). Any status outside
    `expect`, and any call that ends without an HTTP status at all, stops the
    script, so no caller can take a failure for an answer."""
    command = ["gh", "api", "--include", "--method", method, path]
    for name, value in (fields or {}).items():
        command += ["--raw-field", f"{name}={value}"]
    try:
        result = subprocess.run(command, capture_output=True, text=True)
    except OSError as error:
        raise SystemExit(f"could not run gh: {error}")
    # --include puts the status line and the headers before the body.
    head, _, body = result.stdout.partition("\n\n")
    line = STATUS.match(head)
    if line is None:
        raise SystemExit(
            f"{method} {path}: no answer from GitHub (gh exit {result.returncode}): "
            f"{result.stderr.strip() or 'no error output'}")
    status = int(line[1])
    try:
        data = json.loads(body) if body.strip() else None
    except ValueError:
        # A gateway's HTML page, say. Callers check the shape of what they read.
        data = None
    # gh exits non-zero for every HTTP error, expected or not; a success
    # status from a gh that failed is a contradiction and is not trusted.
    if status not in expect or (status < 300 and result.returncode != 0):
        said = data.get("message") if isinstance(data, dict) else None
        raise SystemExit(f"{method} {path}: {meaning(status)}: {said or result.stderr.strip() or 'no detail'}")
    return status, data


def meaning(status):
    if status == 401:
        return "HTTP 401, the token was rejected"
    if status == 403:
        return "HTTP 403, the token may not do this (or is rate limited)"
    if status == 404:
        return "HTTP 404, not found or not visible to this token"
    if status >= 500:
        return f"HTTP {status}, GitHub failed (the request may still have taken effect; a rerun is safe)"
    return f"unexpected HTTP {status}"


def listing(path):
    """Every item of a paginated list."""
    items = []
    for page in range(1, MAX_PAGES + 1):
        _, data = request("GET", f"{path}?per_page={PAGE}&page={page}")
        if not isinstance(data, list):
            raise SystemExit(f"GET {path}: expected a list, got {json.dumps(data)[:200]}")
        items += data
        if len(data) < PAGE:
            return items
    raise SystemExit(f"GET {path}: more than {MAX_PAGES} pages")


def tag_target(repository, tag):
    """None when the tag is absent, else (type, SHA) of the object it points
    at: ("commit", ...) for a lightweight tag, ("tag", ...) for an annotated one."""
    name = f"refs/tags/{tag}"
    # This endpoint matches by prefix, so longer tag names come back too.
    refs = listing(f"repos/{repository}/git/matching-refs/tags/{tag}")
    for ref in refs:
        target = ref.get("object") if isinstance(ref, dict) else None
        if (not isinstance(target, dict) or not isinstance(ref.get("ref"), str)
                or not isinstance(target.get("type"), str) or not isinstance(target.get("sha"), str)):
            raise SystemExit(f"unexpected description of a ref: {json.dumps(ref)[:200]}")
    found = [ref["object"] for ref in refs if ref["ref"] == name]
    if len(found) > 1:
        raise SystemExit(f"GitHub listed {name} {len(found)} times")
    return (found[0]["type"], found[0]["sha"]) if found else None


def require_owned(target, tag, sha, refusal):
    """The tag is the rehearsal's own: lightweight and at the expected commit."""
    if target is None:
        raise SystemExit(f"tag {tag} is absent: {refusal}")
    kind, actual = target
    if kind != "commit":
        raise SystemExit(f"tag {tag} is not a lightweight tag (it points at {kind} {actual}): {refusal}")
    if actual != sha:
        raise SystemExit(f"tag {tag} points at {actual}, not at {sha}: {refusal}")


def releases_of(repository, tag):
    """Every release of the tag, drafts included: [(id, is draft)]. A draft is
    not reachable by its tag name, only through the list of all releases."""
    found = []
    for release in listing(f"repos/{repository}/releases"):
        if (not isinstance(release, dict) or type(release.get("id")) is not int
                or not isinstance(release.get("draft"), bool) or not isinstance(release.get("tag_name"), str)):
            raise SystemExit(f"unexpected description of a release: {json.dumps(release)[:200]}")
        if release["tag_name"] == tag:
            found.append((release["id"], release["draft"]))
    return found


def create(repository, tag, sha):
    target = tag_target(repository, tag)
    if target is not None:
        require_owned(target, tag, sha, "refusing to reuse or move it")
        print(f"tag {tag} already points at {sha}: nothing to create")
        return
    # The listing above answered, so the repository is visible and this 404 is
    # about the commit. Checked first so the tag is never made on anything else.
    status, _ = request("GET", f"repos/{repository}/git/commits/{sha}", expect=(200, 404))
    if status == 404:
        raise SystemExit(f"{sha} is not a commit in {repository}: not creating tag {tag}")
    request("POST", f"repos/{repository}/git/refs", {"ref": f"refs/tags/{tag}", "sha": sha}, expect=(201,))
    require_owned(tag_target(repository, tag), tag, sha, "it is not what was just created")
    print(f"created tag {tag} at {sha}")


def delete_draft(repository, tag, identifier):
    """Deletes one release, only if it is still a draft of this tag right now."""
    path = f"repos/{repository}/releases/{identifier}"
    _, release = request("GET", path)
    if (not isinstance(release, dict) or release.get("id") != identifier
            or release.get("tag_name") != tag or release.get("draft") is not True):
        raise SystemExit(f"release {identifier} is no longer a draft of {tag}: not deleting it or the tag")
    request("DELETE", path, expect=(204,))
    print(f"deleted draft release {identifier} of {tag}")


def cleanup(repository, tag, sha):
    target = tag_target(repository, tag)
    if target is not None:
        require_owned(target, tag, sha, "it is not this rehearsal's, nothing deleted")
    releases = releases_of(repository, tag)
    public = [identifier for identifier, draft in releases if not draft]
    if public:
        raise SystemExit(
            f"release {public[0]} of {tag} is public: a public release and its tag are never deleted, nothing deleted")
    if target is None:
        if releases:
            raise SystemExit(
                f"tag {tag} is absent but draft release {releases[0][0]} names it: without the tag the draft "
                "cannot be shown to be this rehearsal's, nothing deleted. Delete it by hand if it is.")
        print(f"no tag or release named {tag}: nothing to clean up")
        return
    # Drafts first, and the tag only once none is left: the tag is what proves
    # ownership, so a rerun after a failure here can still prove it.
    for identifier, _ in releases:
        delete_draft(repository, tag, identifier)
    left = releases_of(repository, tag)
    if left:
        raise SystemExit(f"release {left[0][0]} of {tag} still exists after the deletions were accepted: tag kept")
    require_owned(tag_target(repository, tag), tag, sha, "it changed during the cleanup, tag not deleted")
    request("DELETE", f"repos/{repository}/git/refs/tags/{tag}", expect=(204,))
    print(f"deleted tag {tag}")
    if tag_target(repository, tag) is not None:
        raise SystemExit(f"tag {tag} still exists after its deletion was accepted")
    if releases_of(repository, tag):
        raise SystemExit(f"a release of {tag} exists after cleanup")
    print(f"verified: no tag or release named {tag} is left")


def main(arguments):
    if len(arguments) != 3 or arguments[0] not in ("create", "cleanup"):
        raise SystemExit(__doc__)
    command, tag, sha = arguments
    if not TAG.fullmatch(tag):
        raise SystemExit(
            f"refusing tag {tag!r}: a rehearsal tag starts with {PREFIX} followed by letters and digits, "
            "which may be separated by single '.', '_' or '-'")
    if not SHA.fullmatch(sha):
        raise SystemExit(f"refusing {sha!r}: the expected commit must be a full 40-digit lowercase SHA")
    repository = os.environ.get("GH_REPO", "")
    if not REPOSITORY.fullmatch(repository):
        raise SystemExit(f"GH_REPO must be owner/name, not {repository!r}")
    (create if command == "create" else cleanup)(repository, tag, sha)


if __name__ == "__main__":
    main(sys.argv[1:])
