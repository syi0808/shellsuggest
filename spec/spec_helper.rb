require 'rspec/wait'
require 'terminal_session'
require 'tempfile'
require 'fileutils'

# Path to the built shellsuggest binary
SHELLSUGGEST_BIN = ENV['SHELLSUGGEST_BIN'] || File.expand_path('../../target/release/shellsuggest', __FILE__)
PLUGIN_PATH = File.expand_path('../../plugin/shellsuggest.plugin.zsh', __FILE__)

RSpec.shared_context 'terminal session' do
  let(:term_opts) { {} }
  let(:session) { TerminalSession.new(term_opts) }
  let(:before_sourcing) { -> {} }
  let(:after_sourcing) { -> {} }
  let(:options) { [] }

  around do |example|
    # Ensure binary is built
    unless File.exist?(SHELLSUGGEST_BIN)
      raise "shellsuggest binary not found at #{SHELLSUGGEST_BIN}. Run `cargo build --release` first."
    end

    # Add binary to PATH
    bin_dir = File.dirname(SHELLSUGGEST_BIN)
    session.run_command("export PATH=\"#{bin_dir}:$PATH\"")

    # Kill any stale daemon and wipe journal for test isolation
    socket_path = "/tmp/shellsuggest-#{Process.uid}.sock"
    pid_path = socket_path.sub('.sock', '.pid')
    if File.exist?(pid_path)
      pid = File.read(pid_path).strip.to_i
      Process.kill('TERM', pid) rescue nil
      sleep 0.2
    end
    File.delete(socket_path) rescue nil
    File.delete(pid_path) rescue nil

    # Wipe the journal DB for clean test state
    db_path = File.join(ENV['XDG_DATA_HOME'] || File.expand_path('~/.local/share'), 'shellsuggest', 'journal.db')
    File.delete(db_path) if File.exist?(db_path)

    before_sourcing.call
    session.run_command(['source ' + PLUGIN_PATH, *options].join('; '))
    after_sourcing.call
    session.clear_screen

    example.run

    # Cleanup daemon after test
    if File.exist?(socket_path)
      pid_path = socket_path.sub('.sock', '.pid')
      if File.exist?(pid_path)
        pid = File.read(pid_path).strip.to_i
        Process.kill('TERM', pid) rescue nil
      end
      File.delete(socket_path) rescue nil
    end

    session.destroy
  end

  # Populate shell history and daemon journal for the test.
  # Journal entries are written directly to SQLite (no daemon needed for writes).
  # Shell history is set via fc -p / fc -R.
  def with_history(*commands, &block)
    # Write journal entries directly to the daemon's SQLite DB
    inject_journal_entries(commands.map { |cmd| { command: cmd, cwd: Dir.pwd } })

    # History file for zsh fc
    hist_file = Tempfile.create('shellsuggest-hist')
    hist_file.write(commands.map { |c| c.gsub("\n", "\\\n") }.join("\n"))
    hist_file.flush

    session.run_command('fc -p')
    session.run_command("fc -R #{hist_file.path}")
    session.clear_screen

    yield block

    session.send_keys('C-c')
    session.run_command('fc -P')

    hist_file.close rescue nil
  end

  # Populate shellsuggest journal directly (no shell history).
  def with_journal(*entries, &block)
    inject_journal_entries(entries)
    session.clear_screen

    yield block
  end

  private

  # Inject journal entries directly via the daemon's Unix socket from Ruby.
  # This avoids all shell quoting issues — protocol frames are sent raw over the socket.
  def inject_journal_entries(entries)
    require 'socket'

    socket_path = "/tmp/shellsuggest-#{Process.uid}.sock"

    # Wait for daemon socket to appear (it may still be starting)
    20.times do
      break if File.exist?(socket_path)
      sleep 0.1
    end
    return unless File.exist?(socket_path)

    begin
      sock = UNIXSocket.new(socket_path)
      entries.each do |entry|
        cmd = entry[:command]
        cwd = entry[:cwd] || Dir.pwd
        exit_code = entry[:exit_code] || 0
        frame = ['r', exit_code.to_s, '10', protocol_escape('test'), protocol_escape(cwd), protocol_escape(cmd)].join("\t")
        sock.puts(frame)
        # Read ack
        sock.gets
      end
      sock.close
    rescue => e
      # Silently fail — daemon might not be ready yet
    end

    sleep 0.1
  end

  def protocol_escape(value)
    value.to_s
         .gsub('\\', '\\\\')
         .gsub("\t", '\\t')
         .gsub("\n", '\\n')
         .gsub("\r", '\\r')
  end
end

RSpec.configure do |config|
  config.expect_with :rspec do |expectations|
    expectations.include_chain_clauses_in_custom_matcher_descriptions = true
  end

  config.mock_with :rspec do |mocks|
    mocks.verify_partial_doubles = true
  end

  config.wait_timeout = 3

  config.include_context 'terminal session'
end
