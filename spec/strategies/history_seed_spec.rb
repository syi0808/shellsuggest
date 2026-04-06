require 'shellwords'

describe 'HISTFILE prewarm' do
  let(:histfile) { Tempfile.new('shellsuggest-histfile-seed') }
  let(:hist_lines) { [] }
  let(:before_sourcing) do
    lambda do
      histfile.write(hist_lines.join("\n"))
      histfile.write("\n") unless hist_lines.empty?
      histfile.flush
      session.run_command("export HISTFILE=#{Shellwords.escape(histfile.path)}")
    end
  end

  after do
    histfile.close!
  rescue StandardError
    nil
  end

  context 'for regular commands' do
    let(:hist_lines) { [': 1712460000:0;echo seeded-from-histfile'] }

    it 'suggests from HISTFILE before the live journal has any matching command' do
      session.send_string('z')
      sleep 0.3
      session.send_keys('C-c')
      session.clear_screen

      session.send_string('echo seeded-f')
      wait_for { session.content }.to eq('echo seeded-from-histfile')
      session.send_keys('C-c')
    end
  end

  context 'for cd commands' do
    let(:hist_lines) { [': 1712460000:0;cd shellsuggest-histfile-only-dir-12345'] }

    it 'does not leak HISTFILE seeded cd entries into cd suggestions' do
      session.send_string('z')
      sleep 0.3
      session.send_keys('C-c')
      session.clear_screen

      session.send_string('cd shellsuggest-histfile-only-d')
      sleep 0.5
      expect(session.content).to eq('cd shellsuggest-histfile-only-d')
      session.send_keys('C-c')
    end
  end
end
