require 'open3'
require 'shellwords'

describe 'plugin state refresh' do
  def run_plugin_script(script)
    env = { 'PATH' => "#{File.dirname(SHELLSUGGEST_BIN)}:#{ENV.fetch('PATH')}" }
    Open3.capture3(env, 'zsh', '-dfic', script)
  end

  it 'recomputes ghost text suffix and keeps dim highlight when the next response is unavailable' do
    script = <<~ZSH
      source #{Shellwords.escape(PLUGIN_PATH)} >/dev/null 2>&1
      _shellsuggest_send_message() { return 0 }
      _shellsuggest_read_suggestion() { return 1 }
      region_highlight=()
      BUFFER='echo h'
      CURSOR=${#BUFFER}
      _SHELLSUGGEST_LAST_BUFFER=''
      _shellsuggest_set_suggestion 'echo hello world' history 0.9 1 0
      BUFFER='echo he'
      CURSOR=${#BUFFER}
      _shellsuggest_suggest || true
      print -r -- "POSTDISPLAY=${POSTDISPLAY}"
      print -r -- "SUGGESTION=${_SHELLSUGGEST_SUGGESTION}"
      print -r -- "HIGHLIGHT=${_SHELLSUGGEST_REGION_HIGHLIGHT}"
    ZSH

    stdout, stderr, status = run_plugin_script(script)

    expect(status.success?).to be(true), stderr
    expect(stdout).to include("POSTDISPLAY=llo world\n")
    expect(stdout).to include("SUGGESTION=echo hello world\n")
    expect(stdout).to match(/HIGHLIGHT=\d+ \d+ .*memo=shellsuggest/)
  end

  it 'clears stale ghost text when the buffer no longer matches the existing suggestion' do
    script = <<~ZSH
      source #{Shellwords.escape(PLUGIN_PATH)} >/dev/null 2>&1
      _shellsuggest_send_message() { return 0 }
      _shellsuggest_read_suggestion() { return 1 }
      region_highlight=()
      BUFFER='echo h'
      CURSOR=${#BUFFER}
      _SHELLSUGGEST_LAST_BUFFER=''
      _shellsuggest_set_suggestion 'echo hello world' history 0.9 1 0
      BUFFER='echo hz'
      CURSOR=${#BUFFER}
      _shellsuggest_suggest || true
      print -r -- "POSTDISPLAY=${POSTDISPLAY}"
      print -r -- "SUGGESTION=${_SHELLSUGGEST_SUGGESTION}"
      print -r -- "HIGHLIGHT=${_SHELLSUGGEST_REGION_HIGHLIGHT}"
    ZSH

    stdout, stderr, status = run_plugin_script(script)

    expect(status.success?).to be(true), stderr
    expect(stdout).to include("POSTDISPLAY=\n")
    expect(stdout).to include("SUGGESTION=\n")
    expect(stdout).to include("HIGHLIGHT=\n")
  end
end
