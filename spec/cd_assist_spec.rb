# Tests for cd cold-start fallback behavior

describe 'cd assist fallback' do
  before do
    @tmpdir = Dir.mktmpdir('shellsuggest-cd-assist-')
    FileUtils.mkdir_p(File.join(@tmpdir, 'src'))
    FileUtils.mkdir_p(File.join(@tmpdir, 'scripts'))
    session.run_command("cd #{@tmpdir}")
    session.clear_screen
  end

  after do
    FileUtils.rm_rf(@tmpdir) if @tmpdir
  end

  it 'suggests direct child directories when local cd history is empty' do
    session.send_string('cd s')
    wait_for { session.content }.to satisfy { |value| ['cd src', 'cd scripts'].include?(value) }
  end

  it 'prefers cwd history over filesystem fallback when history exists' do
    inject_journal_entries([
      { command: 'cd service-old', cwd: @tmpdir },
    ])
    session.clear_screen

    session.send_string('cd s')
    wait_for { session.content }.to eq('cd service-old')
  end
end
