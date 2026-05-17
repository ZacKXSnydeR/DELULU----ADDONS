const { spawn } = require('child_process');
const child = spawn('target/release/spectre.exe', []);
child.stdout.on('data', d => {
    console.log(d.toString());
    child.kill();
});
child.stderr.on('data', d => console.error(d.toString()));
child.stdin.write(JSON.stringify({id: 1, method: "resolveStream", params: {tmdbId: "945961", mediaType: "movie", season: 1, episode: 1}}) + '\n');
