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

    # Wipe the journal DB for clean test state
    db_path = File.join(ENV['XDG_DATA_HOME'] || File.expand_path('~/.local/share'), 'shellsuggest', 'journal.db')
    File.delete(db_path) if File.exist?(db_path)

    before_sourcing.call
    session.run_command(['source ' + PLUGIN_PATH, *options].join('; '))
    after_sourcing.call
    session.clear_screen

    example.run

    session.destroy
  end

  # Populate shell history and shellsuggest journal for the test.
  # Shell history is set via fc -p / fc -R.
  def with_history(*commands, &block)
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

  # Inject journal entries through a standalone query process so the tests use
  # the same protocol path as the zsh plugin without needing shell quoting.
  def inject_journal_entries(entries)
    begin
      IO.popen([SHELLSUGGEST_BIN, 'query'], 'r+') do |io|
        entries.each do |entry|
          cmd = entry[:command]
          cwd = entry[:cwd] || Dir.pwd
          exit_code = entry[:exit_code] || 0
          frame = ['r', exit_code.to_s, '10', protocol_escape('test'), protocol_escape(cwd), protocol_escape(cmd)].join("\t")
          io.puts(frame)
          io.flush
          io.gets
        end
      end
    rescue StandardError
      entries.each do |entry|
        warn "failed to inject journal entry for #{entry[:command].inspect}"
      end
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
