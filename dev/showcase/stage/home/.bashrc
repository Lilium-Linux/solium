# The showcase's shell: a prompt with no user or host in it, so a capture
# shows the work and nothing about the machine it was made on.
PS1='\[\e[38;2;147;109;255m\]❯\[\e[0m\] '
PROMPT_COMMAND=
HISTFILE=/dev/null
export LESS=-R
