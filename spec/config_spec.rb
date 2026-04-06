# Tests config.toml overrides

describe 'config overrides' do
  let(:before_sourcing) do
    lambda do
      @config_root = Dir.mktmpdir('shellsuggest-config-')
      config_dir = File.join(@config_root, 'shellsuggest')
      FileUtils.mkdir_p(config_dir)
      File.write(File.join(config_dir, 'config.toml'), <<~TOML)
        [path]
        show_hidden = true
      TOML

      session.run_command("export XDG_CONFIG_HOME=#{@config_root}")
    end
  end

  after do
    FileUtils.rm_rf(@config_root) if @config_root
  end

  it 'shows hidden directories when enabled in config.toml' do
    Dir.mktmpdir('shellsuggest-config-workspace') do |dir|
      FileUtils.mkdir_p(File.join(dir, '.hidden'))
      FileUtils.mkdir_p(File.join(dir, 'src'))

      session.run_command("cd #{dir}")
      session.clear_screen

      session.send_string('pushd ')
      wait_for { session.content }.to include('.hidden')
    end
  end
end
