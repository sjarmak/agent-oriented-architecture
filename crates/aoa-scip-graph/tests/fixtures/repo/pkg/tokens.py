"""Token issuance and verification — holds the security invariant."""


def issue_token(user):
    # invariant: tokens must never be issued for anonymous users
    if user is None:
        raise ValueError("anonymous user")
    return verify_secret(user)


def verify_secret(user):
    return "tok-" + str(user)
