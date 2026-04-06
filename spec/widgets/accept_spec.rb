# Tests for suggestion accept widgets

describe 'accept widgets' do
  describe 'right arrow (full accept)' do
    it 'accepts the entire suggestion' do
      with_history('echo hello world') do
        session.send_string('echo h')
        wait_for { session.content }.to eq('echo hello world')

        session.send_keys('Right')
        wait_for { session.content }.to eq('echo hello world')
        wait_for { session.cursor }.to eq([16, 0])
      end
    end

    it 'moves cursor forward when there is no suggestion' do
      session.send_string('abc')
      session.send_keys('Left', 'Left') # cursor at 'a|bc'
      session.send_keys('Right')        # should just move forward
      wait_for { session.cursor }.to eq([2, 0])
    end
  end

  describe 'ctrl+right arrow (word accept)' do
    it 'accepts one word from the suggestion' do
      with_history('echo hello world') do
        session.send_string('echo')
        wait_for { session.content }.to eq('echo hello world')

        session.send_keys('C-Right')
        # Should accept ' hello ' (next word)
        wait_for { session.content }.to start_with('echo hello')
      end
    end
  end

  describe 'alt+f (macOS-safe word accept)' do
    it 'accepts one word from the suggestion without relying on ctrl+arrow' do
      with_history('echo hello world') do
        session.send_string('echo')
        wait_for { session.content }.to eq('echo hello world')

        session.send_keys('M-f')
        wait_for { session.content }.to start_with('echo hello')
      end
    end
  end

  describe 'escape (clear)' do
    it 'clears the suggestion' do
      with_history('echo hello') do
        session.send_string('echo')
        wait_for { session.content }.to eq('echo hello')

        session.send_keys('escape')
        wait_for { session.content }.to eq('echo')
      end
    end
  end

  describe 'highlight cleanup' do
    it 'removes dim styling after accepting the suggestion' do
      with_history('echo hello world') do
        session.send_string('echo h')
        wait_for { session.content }.to eq('echo hello world')
        expect(session.content(esc_seqs: true)).to include("\e[")

        session.send_keys('Right')
        wait_for { session.content }.to eq('echo hello world')
        expect(session.content(esc_seqs: true)).to eq('echo hello world')
      end
    end
  end
end
