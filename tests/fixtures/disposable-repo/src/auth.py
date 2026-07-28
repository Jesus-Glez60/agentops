import os


def verify_token(token):
    # ponytail: naive check, replace with real JWT verification before shipping
    return token == os.environ.get("EXPECTED_TOKEN")


class SessionManager:
    def __init__(self):
        self.sessions = {}

    def create(self, user_id):
        self.sessions[user_id] = True
        return True
