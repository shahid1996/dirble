use std::{
    collections::VecDeque,
    sync::{Arc, mpsc::{self, Sender, Receiver}},
    thread,
    time::Duration,
};
extern crate curl;
mod arg_parse;
mod request;
mod wordlist;
mod output;

fn main() {
    // Read the arguments in using the arg_parse module
    let global_opts = Arc::new(arg_parse::get_args());

    // Get the wordlist file from the arguments and open it
    let wordlist = Arc::new(wordlist::lines_from_file(global_opts.wordlist_file.clone()).unwrap());

    // Create a queue for URIs that need to be scanned
    let mut scan_queue: VecDeque<wordlist::UriGenerator> = VecDeque::new();

    // Push the host URI to the scan queue
    for extension in global_opts.extensions.clone() {
        for start_index in 0..global_opts.wordlist_split {
            scan_queue.push_back(
                wordlist::UriGenerator::new(global_opts.hostname.clone(), String::from(extension.clone()), wordlist.clone(), 
                    start_index, global_opts.wordlist_split));
        }
    }
    
    // Create a channel for threads to communicate with the parent on
    // This is used to send information about ending threads and information on responses
    let (tx, rx): (Sender<request::RequestResponse>, Receiver<request::RequestResponse>) = mpsc::channel();

    // Define the max number of threads and the number of threads currently in use
    let mut threads_in_use = 0;

    let mut response_list: Vec<request::RequestResponse> = Vec::new();

    let file_handles = output::create_files(global_opts.clone());

    // Loop of checking for messages from the threads,
    // spawning new threads on items in the scan queue
    // and checking if the program is done
    loop {

        // Check for messages from the threads
        let reply = rx.try_recv();
        match reply {
            Ok(message) => {
                // If a thread has sent end, then we can reduce the threads in use count
                if message.url == "END" {
                    threads_in_use -= 1; }

                // If a thread sent anything else, then call the print_response function to deal with output
                // If the response was a directory, create generators with each extension and add it to the scan queue
                else { 
                    if !global_opts.silent {
                        match output::print_response(&message, global_opts.clone(), false) {
                            Some(output) => { println!("{}", output) },
                            None => {}
                        }
                    }
                    if message.is_directory && !message.is_listable && !global_opts.disable_recursion {
                        for extension in global_opts.extensions.clone() {
                            for start_index in 0..global_opts.wordlist_split {
                                scan_queue.push_back(
                                    wordlist::UriGenerator::new(message.url.clone(), String::from(extension.clone()), wordlist.clone(), 
                                        start_index, global_opts.wordlist_split));
                            }
                        }
                    }
                    else if message.is_listable{ println!("{} is listable, skipping scanning", message.redirect_url); }
                    response_list.push(message);
                }
            },
            // Ignore any errors - this happens if the message queue is empty, that's okay
            Err(_) => {},
        };

        // If there are items in the scan queue and available threads
        // Spawn a new thread to scan an item
        if threads_in_use < global_opts.max_threads && scan_queue.len() > 0 {

            // Clone a new sender to the channel and a new wordlist reference
            // Then pop the scan target from the queue
            let tx_clone = mpsc::Sender::clone(&tx);
            let list_gen = scan_queue.pop_front().unwrap();
            let arg_clone = global_opts.clone();

            // Spawn a thread with the arguments and increment the in use counter
            thread::spawn(|| thread_spawn(tx_clone, list_gen, arg_clone));
            threads_in_use += 1;
        }

        // If there are no threads in use and the queue is empty then stop
        if threads_in_use == 0 && scan_queue.len() == 0 {
            break;
        }

        // Sleep to reduce CPU cycles used by main
        thread::sleep(Duration::from_millis(1));
    }

    output::print_report(response_list, global_opts.clone(), file_handles);
}

fn thread_spawn(tx: mpsc::Sender<request::RequestResponse>, uri_gen: wordlist::UriGenerator, global_opts: Arc<arg_parse::GlobalOpts>) {

    let hostname = uri_gen.hostname.clone();

    if global_opts.verbose {
        println!("Scanning {}/", hostname);
    }

    let mut easy = request::generate_easy(global_opts.clone());

    let mut consecutive_errors = 0;

    // For each item in the wordlist, call the request function on it
    // Then if there is a response send it to main
    for uri in uri_gen {
        let req_response = request::make_request(&mut easy, uri.clone());

        match req_response{
            Some(response) => { 
                let code = response.code.clone();
                tx.send(response).unwrap(); 
                if global_opts.max_errors != 0 {
                    if code == 0 {
                        consecutive_errors += 1;
                        if consecutive_errors >= global_opts.max_errors {
                            println!("Thread scanning {} stopping due to multiple consecutive errors received", hostname);

                            let end = request::RequestResponse {
                                url: String::from("END"),
                                code: 0,
                                content_len: 0,
                                is_directory:false,
                                is_listable: false,
                                redirect_url: String::from("")
                            };
                            tx.send(end).unwrap();
                            break;
                        }
                    }
                    else {
                        consecutive_errors = 0;
                    }
                }
            }
            None => {}
        }
        if global_opts.throttle != 0 {
            thread::sleep(Duration::from_millis(global_opts.throttle as u64));
        }
    }

    if global_opts.verbose {
        println!("Finished scanning {}/", hostname);
    }

    // Send a message to the main thread so it knows the thread is done
    let end = request::RequestResponse {
        url: String::from("END"),
        code: 0,
        content_len: 0,
        is_directory:false,
        is_listable: false,
        redirect_url: String::from("")
    };
    tx.send(end).unwrap();
}