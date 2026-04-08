# Tests for cycling through multiple candidates

describe 'candidate cycling' do
  before do
    @tmpdir = Dir.mktmpdir('shellsuggest-cycle-')
    Dir.mkdir(File.join(@tmpdir, 'src'))
    Dir.mkdir(File.join(@tmpdir, 'scripts'))

    session.run_command("cd #{@tmpdir}")
    session.clear_screen
  end

  after do
    FileUtils.rm_rf(@tmpdir) if @tmpdir
  end

  it 'cycles to the next and previous suggestion with alt+j/alt+k' do
    session.send_string('cd s')
    wait_for { session.content }.to satisfy { |value| ['cd src', 'cd scripts'].include?(value) }
    first = session.content

    session.send_keys('M-j')
    wait_for { session.content }.to satisfy { |value| value != first && ['cd src', 'cd scripts'].include?(value) }
    second = session.content

    session.send_keys('M-k')
    wait_for { session.content }.to eq(first)
    expect(second).not_to eq(first)
  end

  it 'wraps around when cycling past either end' do
    session.send_string('cd s')
    wait_for { session.content }.to satisfy { |value| ['cd src', 'cd scripts'].include?(value) }
    first = session.content

    session.send_keys('M-k')
    wait_for { session.content }.to satisfy { |value| value != first && ['cd src', 'cd scripts'].include?(value) }
    wrapped = session.content

    session.send_keys('M-j')
    wait_for { session.content }.to eq(first)
    expect(wrapped).not_to eq(first)
  end
end
