# shellsuggest-specific: Path completion tests

describe 'path suggestion' do
  before do
    @tmpdir = Dir.mktmpdir('shellsuggest-test-')
    Dir.mkdir(File.join(@tmpdir, 'src'))
    Dir.mkdir(File.join(@tmpdir, 'scripts'))
    Dir.mkdir(File.join(@tmpdir, '.hidden'))
    File.write(File.join(@tmpdir, 'readme.md'), 'hello')
    File.write(File.join(@tmpdir, 'Cargo.toml'), '')

    session.run_command("cd #{@tmpdir}")
    session.clear_screen
  end

  after do
    FileUtils.rm_rf(@tmpdir) if @tmpdir
  end

  describe 'pushd completion' do
    it 'suggests only directories' do
      session.send_string('pushd s')
      wait_for { session.content }.to start_with('pushd s')
      # Should suggest src or scripts, not readme.md
      expect(session.content).not_to include('readme')
      expect(session.content).not_to include('Cargo')
    end

    it 'suggests directories matching prefix' do
      session.send_string('pushd sr')
      wait_for { session.content }.to eq('pushd src')
    end
  end

  describe 'vim completion' do
    it 'suggests files matching prefix' do
      session.send_string('vim rea')
      wait_for { session.content }.to eq('vim readme.md')
    end
  end

  describe 'hidden files' do
    it 'does not suggest hidden directories by default' do
      session.send_string('pushd ')
      sleep 0.5
      expect(session.content).not_to include('.hidden')
    end
  end
end
