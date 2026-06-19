"""Auth module for the fixture repo."""

from pkg.tokens import issue_token


def login(user):
    return issue_token(user)


def logout(user):
    return None
