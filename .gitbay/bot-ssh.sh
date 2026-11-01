# Sourced by pages.sh and release.sh. The build runs in a container that
# holds no key of its own; the orgo-ci account's private key (write on this
# repository) arrives as the BOT_SSH_KEY secret and is written into the
# workspace beside an ssh config every ssh and git call is pointed at with
# -F. GITBAY_SSH, set by the runner, is the instance as this build reaches
# it. Nothing is written outside the workspace.
: "${BOT_SSH_KEY:?BOT_SSH_KEY secret is not set}"
: "${GITBAY_SSH:?GITBAY_SSH is not set; the runner is too old}"
(
	umask 077
	printf '%s\n' "$BOT_SSH_KEY" >"$PWD/.bot_key"
	printf 'IdentityFile %s\nIdentitiesOnly yes\nStrictHostKeyChecking accept-new\nUserKnownHostsFile %s\n' \
		"$PWD/.bot_key" "$PWD/.known_hosts" >"$PWD/.ssh_config"
)
SSH="ssh -F $PWD/.ssh_config"
bot_ssh() { ssh -F "$PWD/.ssh_config" "$@"; }
export GIT_SSH_COMMAND="$SSH"
