require 'shellwords'
require 'tmpdir'

# shellsuggest-specific: CWD-aware history ranking
# Tests that commands executed in the current directory are preferred

describe 'cwd-aware history suggestion' do
  it 'prefers commands from the current directory over other directories' do
    # Run a successful command from /tmp
    session.run_command('cd /tmp && printf deploy >/dev/null; cd - >/dev/null')
    sleep 0.3
    # Run a successful command from the current directory
    session.run_command('printf test >/dev/null')
    sleep 0.3
    session.clear_screen

    session.send_string('printf ')
    # Should prefer the cwd-local command over the /tmp one.
    wait_for { session.content }.to start_with('printf test')
  end

  it 'falls back to global history when no cwd match exists' do
    session.run_command('echo globalcmd123')
    sleep 0.3
    session.clear_screen

    session.send_string('echo global')
    wait_for { session.content }.to eq('echo globalcmd123')
  end

  it 'uses the last successful command as transition context' do
    Dir.mktmpdir('shellsuggest-transition-context') do |dir|
      session.run_command("cd #{Shellwords.escape(dir)}")
      session.run_command('touch main.rs')
      session.run_command('vim() { :; }')
      inject_journal_entries([
        { command: 'vim main.rs', cwd: dir },
        { command: 'make test', cwd: dir },
        { command: 'vim main.rs', cwd: dir },
        { command: 'make test', cwd: dir },
        { command: 'vim main.rs', cwd: dir },
        { command: 'make build', cwd: dir },
      ])

      session.run_command('vim main.rs')
      sleep 0.3
      session.clear_screen

      session.send_string('make ')
      wait_for { session.content }.to eq('make test')
    end
  end

  it 'does not fall back to global history or filesystem for cd' do
    session.run_command('cd / && cd tmp >/dev/null; cd - >/dev/null')
    sleep 0.3
    session.clear_screen

    session.send_string('cd t')
    sleep 0.5
    expect(session.content).to eq('cd t')
  end

  it 'does not suggest cd commands from the destination directory' do
    Dir.mktmpdir('shellsuggest-cd-cwd-history') do |dir|
      workspace = File.join(dir, 'Workspace')
      pwd_file = File.join(dir, 'pwd.txt')
      FileUtils.mkdir_p(workspace)

      session.run_command("cd #{Shellwords.escape(dir)}")
      session.run_command('cd Workspace')
      sleep 0.5
      session.run_command("print -r -- $PWD > #{Shellwords.escape(pwd_file)}")
      wait_for { File.exist?(pwd_file) && File.read(pwd_file).strip }.to eq(workspace)
      session.clear_screen

      session.run_command('cd ..')
      sleep 0.5
      session.clear_screen

      session.send_string('cd .')
      sleep 0.5
      expect(session.content).to eq('cd .')
    end
  end
end
