require 'shellwords'
require 'tmpdir'

# shellsuggest-specific: Daemon lifecycle tests

describe 'daemon lifecycle' do
  it 'auto-starts daemon when not running' do
    # Kill any existing daemon first
    session.run_command('shellsuggest status')
    sleep 0.3

    # Type something — daemon should auto-start via coproc
    with_history('echo hello') do
      session.send_string('echo h')
      wait_for { session.content }.to eq('echo hello')
    end
  end

  it 'records executed commands to journal' do
    session.run_command('echo test_marker_12345')
    sleep 0.5

    session.run_command('shellsuggest journal')
    wait_for { session.content }.to include('test_marker_12345')
  end

  it 'records cd commands against the cwd they were executed from' do
    Dir.mktmpdir('shellsuggest-cd-record') do |dir|
      workspace = File.join(dir, 'Workspace')
      FileUtils.mkdir_p(workspace)

      session.run_command("cd #{Shellwords.escape(dir)}")
      session.run_command('cd Workspace')
      session.run_command('cd ..')
      sleep 0.5

      env = { 'PATH' => "#{File.dirname(SHELLSUGGEST_BIN)}:#{ENV.fetch('PATH')}" }
      journal = nil

      wait_for do
        journal = IO.popen(env, [SHELLSUGGEST_BIN, 'journal'], &:read)
      end.to include("cd Workspace (cwd: #{dir}, exit: 0)")

      wait_for do
        journal = IO.popen(env, [SHELLSUGGEST_BIN, 'journal'], &:read)
      end.to include("cd .. (cwd: #{workspace}, exit: 0)")
    end
  end

  it 'status command shows daemon state' do
    # After plugin is loaded, daemon should be running
    sleep 0.5
    session.run_command('shellsuggest status')
    wait_for { session.content }.to include('daemon:')
  end

  it 'shows suggestions in a fresh terminal while reusing an existing daemon' do
    with_history('echo hello') do
      session.send_string('echo h')
      wait_for { session.content }.to eq('echo hello')
      session.send_keys('C-c')
      session.clear_screen

      other = TerminalSession.new(prompt: '')
      begin
        other.run_command("export PATH=\"#{File.dirname(SHELLSUGGEST_BIN)}:$PATH\"")
        other.run_command("source #{PLUGIN_PATH}")
        other.clear_screen
        other.send_string('echo h')
        wait_for { other.content }.to eq('echo hello')
      ensure
        other.destroy
      end
    end
  end
end
