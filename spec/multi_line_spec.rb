# Multi-line / complex suggestion tests

describe 'complex suggestions' do
  it 'suggests commands with special content via journal injection' do
    # Inject directly to daemon — avoids tmux/shell quoting entirely
    inject_journal_entries([{ command: 'grep -r TODO src/', cwd: Dir.pwd }])
    session.clear_screen

    session.send_string('grep -r')
    wait_for { session.content }.to eq('grep -r TODO src/')
  end

  it 'suggests commands with pipes' do
    inject_journal_entries([{ command: 'cat file.txt | sort | uniq', cwd: Dir.pwd }])
    session.clear_screen

    session.send_string('cat f')
    wait_for { session.content }.to eq('cat file.txt | sort | uniq')
  end

  it 'suggests commands with redirections' do
    inject_journal_entries([{ command: 'echo hello > output.txt', cwd: Dir.pwd }])
    session.clear_screen

    session.send_string('echo hello >')
    wait_for { session.content }.to eq('echo hello > output.txt')
  end

  it 'suggests commands with semicolons' do
    inject_journal_entries([{ command: 'cd src; make test', cwd: Dir.pwd }])
    session.clear_screen

    session.send_string('cd src;')
    wait_for { session.content }.to eq('cd src; make test')
  end

  # Note: single/double quote tests are skipped due to tmux send-keys
  # quoting limitations. The daemon correctly handles quotes — verified
  # via direct socket tests in tests/integration.rs
end
